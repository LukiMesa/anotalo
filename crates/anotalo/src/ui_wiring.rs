//! La ventana dentro de la app de la bandeja (spec §5.6): los avisos de la app como eventos de la
//! página (los mismos que mandaba la app de Python), las órdenes de la página hacia `UiApi` y lo
//! que los hilos de las órdenes piden al hilo principal (los atajos, los diálogos y la ventana son
//! suyos).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, ThreadId};
use std::time::Duration;

use anotalo_core::app::{AppEvent, SessionKind};
use anotalo_core::hotkey::{parse_hotkey, validate_hotkey};
use anotalo_core::model::ModelStatus;
use anotalo_core::models::{Placement, model_for};
use anotalo_core::ui_api::{UiApi, kind_label};
use serde_json::{Value, json};

use crate::ui_bridge::Commands;

/// El aviso si la ventana no se puede abrir (la app sigue en la bandeja).
pub const NO_WINDOW_TITLE: &str = "No se pudo abrir la ventana";

pub fn no_window_body() -> String {
    format!(
        "Instala WebView2: {}",
        anotalo_winsys::webview2::DOWNLOAD_URL
    )
}

impl Commands for UiApi {
    fn call(&self, method: &str, args: &[Value]) -> Value {
        UiApi::call(self, method, args)
    }
}

/// Lo que los eventos de la página cuentan de la app en ese momento.
pub struct PageCtx {
    /// El archivo que se está grabando (su nombre es el id de la página).
    pub live_id: Option<String>,
    pub label_me: String,
    pub hotkey: String,
    pub device: String,
    /// La primera preparación del modelo: (progreso 0–1, texto).
    pub preparing: Option<(f32, String)>,
    pub kind: Option<SessionKind>,
}

/// El evento de la página para un aviso de la app (None: la página no lo usa).
pub fn page_event(e: &AppEvent, ctx: &PageCtx) -> Option<Value> {
    match e {
        AppEvent::State(s) => Some(json!({
            "type": "state",
            "state": s.as_str(),
            "hotkey": ctx.hotkey,
            "device": ctx.device,
            "progress": ctx.preparing.as_ref().map(|(p, _)| *p),
            "progress_text": ctx.preparing.as_ref().map_or("", |(_, t)| t.as_str()),
            "kind": ctx.kind.map(kind_label),
        })),
        AppEvent::Line {
            speaker, hms, text, ..
        } => Some(json!({
            "type": "line",
            "id": ctx.live_id.as_ref()?,
            "ts": hms,
            "speaker": speaker,
            "me": *speaker == ctx.label_me,
            "text": text,
        })),
        AppEvent::Marked { hms } => Some(json!({
            "type": "line",
            "id": ctx.live_id.as_ref()?,
            "ts": hms,
            "speaker": "",
            "me": false,
            "text": "",
            "mark": true,
        })),
        AppEvent::SessionStarted { .. } => {
            Some(json!({"type": "recordings", "select": ctx.live_id}))
        }
        AppEvent::SessionEnded { path, .. } => {
            let name = path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            Some(json!({"type": "recordings", "select": name}))
        }
        AppEvent::Level(_) | AppEvent::SessionStopping { .. } => None,
    }
}

/// Un atajo nuevo desde la ventana: válido y distinto del otro atajo de la app (`other`, el de
/// `other_name`: "grabar" o "marcar").
pub fn check_hotkey(spec: &str, other: &str, other_name: &str) -> Result<(), String> {
    let new = validate_hotkey(spec)?;
    if parse_hotkey(other).is_ok_and(|o| o == new) {
        return Err(format!("Ese atajo ya es el de {other_name}"));
    }
    Ok(())
}

/// Qué muestra la ventana del modelo (como `device_info` de Python): dónde corre y cuál es.
/// `loaded`: dónde se cargó (None sin modelo); `chosen`: dónde se cargará; `gpu`: el nombre de esa
/// GPU; `model_setting`: el ajuste `model`.
pub fn device_label(
    status: &ModelStatus,
    loaded: Option<Placement>,
    chosen: Placement,
    gpu: Option<&str>,
    model_setting: &str,
) -> String {
    let rest = match status {
        ModelStatus::Preparing { .. } => return "Cargando modelo…".into(),
        ModelStatus::Failed(_) => return "Modelo no disponible".into(),
        ModelStatus::Ready => "",
        ModelStatus::Released => " · en reposo",
    };
    let at = loaded.unwrap_or(chosen);
    let model = model_for(model_setting, matches!(at, Placement::Gpu { .. })).setting;
    let gpu = gpu.unwrap_or("GPU");
    match at {
        Placement::Gpu { .. } => format!("GPU · {gpu} · {model}{rest}"),
        Placement::IntegratedGpu { .. } => format!("GPU integrada · {gpu} · {model}{rest}"),
        // en el procesador porque la GPU falló al cargar
        Placement::Cpu if chosen != Placement::Cpu => {
            format!("Procesador (respaldo) · {model}{rest}")
        }
        Placement::Cpu => format!("Procesador · {model}{rest}"),
    }
}

const PENDING: u8 = 0;
const RUNNING: u8 = 1;
const CANCELLED: u8 = 2;

struct Job<T: 'static> {
    run: Box<dyn FnOnce(&T) + Send>,
    state: Arc<AtomicU8>,
}

/// Lo que otros hilos piden hacer en el hilo principal. `wake` le avisa de que hay algo
/// (`PostMessage`); el hilo principal lo hace con `run_pending`.
pub struct MainCalls<T: 'static> {
    queue: Mutex<VecDeque<Job<T>>>,
    wake: Box<dyn Fn() + Send + Sync>,
    main: ThreadId,
    closed: AtomicBool,
}

impl<T: 'static> MainCalls<T> {
    /// Se crea en el hilo principal.
    pub fn new(wake: Box<dyn Fn() + Send + Sync>) -> MainCalls<T> {
        MainCalls {
            queue: Mutex::new(VecDeque::new()),
            wake,
            main: thread::current().id(),
            closed: AtomicBool::new(false),
        }
    }

    /// Desde otro hilo: hace `f` en el hilo principal y devuelve lo que dé. Con `timeout`, si el
    /// hilo principal no la empieza a tiempo no se hace nunca, y None. None también si la app ya
    /// se cierra.
    pub fn call<R: Send + 'static>(
        &self,
        timeout: Option<Duration>,
        f: impl FnOnce(&T) -> R + Send + 'static,
    ) -> Option<R> {
        if thread::current().id() == self.main {
            log::error!("Llamada al hilo principal desde él mismo: no se hace");
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(AtomicU8::new(PENDING));
        {
            let mut q = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            // con el cerrojo: `close` no deja nada en la cola que nadie vaya a hacer
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            q.push_back(Job {
                run: Box::new(move |t| {
                    let _ = tx.send(f(t));
                }),
                state: state.clone(),
            });
        }
        (self.wake)();
        let Some(timeout) = timeout else {
            return rx.recv().ok(); // Err: se cerró sin hacerla
        };
        match rx.recv_timeout(timeout) {
            Ok(r) => Some(r),
            // si ya empezó, se espera a que termine; si no, ya no se hará
            Err(_) => {
                match state.compare_exchange(PENDING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
                {
                    Ok(_) => {
                        log::warn!("El hilo principal no atendió a tiempo: se abandona la llamada");
                        None
                    }
                    Err(_) => rx.recv().ok(),
                }
            }
        }
    }

    /// Desde cualquier hilo: que el hilo principal haga `f` en cuanto pueda, sin esperarlo.
    pub fn post(&self, f: impl FnOnce(&T) + Send + 'static) {
        {
            let mut q = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            q.push_back(Job {
                run: Box::new(f),
                state: Arc::new(AtomicU8::new(PENDING)),
            });
        }
        (self.wake)();
    }

    /// En el hilo principal: hace lo pendiente, en orden.
    pub fn run_pending(&self, target: &T) {
        loop {
            // sin el cerrojo mientras se hace: un diálogo mueve mensajes y puede volver aquí
            let job = self
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            let Some(job) = job else { break };
            if job
                .state
                .compare_exchange(PENDING, RUNNING, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                (job.run)(target);
            }
        }
    }

    /// Al salir: lo pendiente ya no se hace y lo nuevo tampoco (quien espera recibe None).
    pub fn close(&self) {
        let dropped: Vec<Job<T>> = {
            let mut q = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            self.closed.store(true, Ordering::SeqCst);
            q.drain(..).collect()
        };
        drop(dropped); // quien esperaba recibe None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anotalo_core::app::State;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::time::Instant;

    fn ctx() -> PageCtx {
        PageCtx {
            live_id: Some("2026-11-12 10-00 Teams.md".into()),
            label_me: "Yo".into(),
            hotkey: "<ctrl>+<alt>+r".into(),
            device: "GPU · RTX 3060 · large-v3-turbo".into(),
            preparing: None,
            kind: Some(SessionKind::Call),
        }
    }

    /// Los mismos eventos que mandaba la app de Python (calltx/main.py), más las marcas en vivo.
    #[test]
    fn test_events_map_to_page_payloads() {
        let c = ctx();
        assert_eq!(
            page_event(&AppEvent::State(State::Recording), &c),
            Some(json!({
                "type": "state", "state": "recording", "hotkey": "<ctrl>+<alt>+r",
                "device": "GPU · RTX 3060 · large-v3-turbo", "progress": null,
                "progress_text": "", "kind": "call",
            }))
        );
        let preparing = PageCtx {
            preparing: Some((0.25, "Descargando el modelo…".into())),
            kind: None,
            ..ctx()
        };
        assert_eq!(
            page_event(&AppEvent::State(State::Preparing), &preparing),
            Some(json!({
                "type": "state", "state": "preparing", "hotkey": "<ctrl>+<alt>+r",
                "device": "GPU · RTX 3060 · large-v3-turbo", "progress": 0.25,
                "progress_text": "Descargando el modelo…", "kind": null,
            }))
        );
        let line = |speaker: &str| AppEvent::Line {
            speaker: speaker.into(),
            hms: "00:01:15".into(),
            start_s: 75.0,
            text: "Hola".into(),
        };
        assert_eq!(
            page_event(&line("Yo"), &c),
            Some(json!({
                "type": "line", "id": "2026-11-12 10-00 Teams.md", "ts": "00:01:15",
                "speaker": "Yo", "me": true, "text": "Hola",
            }))
        );
        assert_eq!(page_event(&line("Ana"), &c).unwrap()["me"], json!(false));
        assert_eq!(
            page_event(
                &AppEvent::Marked {
                    hms: "00:02:00".into()
                },
                &c
            ),
            Some(json!({
                "type": "line", "id": "2026-11-12 10-00 Teams.md", "ts": "00:02:00",
                "speaker": "", "me": false, "text": "", "mark": true,
            }))
        );
        // sin grabación en curso no hay a qué añadir la frase
        let idle = PageCtx {
            live_id: None,
            ..ctx()
        };
        assert_eq!(page_event(&line("Yo"), &idle), None);
        assert_eq!(
            page_event(
                &AppEvent::SessionStarted {
                    label: "Teams".into()
                },
                &c
            ),
            Some(json!({"type": "recordings", "select": "2026-11-12 10-00 Teams.md"}))
        );
        let ended = |path: Option<PathBuf>| AppEvent::SessionEnded {
            path,
            label: "Teams".into(),
            copied: false,
        };
        assert_eq!(
            page_event(
                &ended(Some(PathBuf::from(
                    r"C:\Llamadas\2026-11-12 10-00 Teams.md"
                ))),
                &idle
            ),
            Some(json!({"type": "recordings", "select": "2026-11-12 10-00 Teams.md"}))
        );
        assert_eq!(
            page_event(&ended(None), &idle),
            Some(json!({"type": "recordings", "select": null}))
        );
        assert_eq!(page_event(&AppEvent::Level(0.5), &c), None);
        assert_eq!(
            page_event(
                &AppEvent::SessionStopping {
                    label: "Teams".into()
                },
                &c
            ),
            None
        );
    }

    #[test]
    fn test_mark_hotkey_must_differ_from_record() {
        assert_eq!(
            check_hotkey("<ctrl>+<alt>+k", "<ctrl>+<alt>+r", "grabar"),
            Ok(())
        );
        assert_eq!(
            check_hotkey("<alt>+<ctrl>+r", "<ctrl>+<alt>+r", "grabar"),
            Err("Ese atajo ya es el de grabar".to_string())
        );
        assert_eq!(
            check_hotkey("<shift>+r", "<ctrl>+<alt>+m", "marcar"),
            Err("El atajo necesita Ctrl, Alt o Win".to_string())
        );
        // si el otro está dañado en el archivo, no estorba
        assert_eq!(check_hotkey("<ctrl>+<alt>+r", "nada", "marcar"), Ok(()));
    }

    #[test]
    fn test_device_label_like_python() {
        let gpu = Placement::Gpu { device: 0 };
        let label =
            |s: &ModelStatus, loaded, chosen, name| device_label(s, loaded, chosen, name, "auto");
        let preparing = ModelStatus::Preparing {
            progress: 0.0,
            text: "Cargando el modelo…".into(),
        };
        assert_eq!(label(&preparing, None, gpu, None), "Cargando modelo…");
        assert_eq!(
            label(&ModelStatus::Ready, Some(gpu), gpu, Some("RTX 3060")),
            "GPU · RTX 3060 · large-v3-turbo"
        );
        assert_eq!(
            label(&ModelStatus::Released, None, gpu, Some("RTX 3060")),
            "GPU · RTX 3060 · large-v3-turbo · en reposo"
        );
        assert_eq!(
            label(
                &ModelStatus::Ready,
                Some(Placement::Cpu),
                gpu,
                Some("RTX 3060")
            ),
            "Procesador (respaldo) · small"
        );
        let igpu = Placement::IntegratedGpu { device: 0 };
        assert_eq!(
            label(&ModelStatus::Ready, Some(igpu), igpu, Some("Intel Iris Xe")),
            "GPU integrada · Intel Iris Xe · small"
        );
        assert_eq!(
            device_label(
                &ModelStatus::Ready,
                Some(Placement::Cpu),
                Placement::Cpu,
                None,
                "medium"
            ),
            "Procesador · medium"
        );
        assert_eq!(
            label(&ModelStatus::Failed("x".into()), None, gpu, None),
            "Modelo no disponible"
        );
    }

    /// La página de verdad en WebView2, con `UiApi` sobre la carpeta de ejemplo de la biblioteca:
    /// la lista tiene todas las grabaciones y el detalle de la de Acme muestra su título, su
    /// cliente, las personas y la fila de la marca.
    #[test]
    #[ignore = "necesita WebView2 y una sesión de escritorio"]
    fn page_renders_library_and_detail() {
        use crate::shell::main_window::{MainWindow, initial_rect};
        use crate::ui_bridge::{Bridge, Reply, reply_js};
        use anotalo_core::app::{App, AppDeps, DetectorFactory};
        use anotalo_core::config::Settings;
        use anotalo_core::detector::DetectorControl;
        use anotalo_core::model::ModelGate;
        use anotalo_core::notify::Notifier;
        use anotalo_core::paths::Profile;
        use anotalo_core::ui_api::{UiDeps, WindowControl};
        use std::path::Path;
        use std::rc::Rc;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
            TranslateMessage,
        };

        struct Quiet;
        impl Notifier for Quiet {
            fn notify(&self, _: &str, _: &str, _: Option<&Path>) {}
        }
        impl DetectorControl for Quiet {
            fn start(&self) {}
            fn stop(&self) {}
            fn suspend(&self) {}
            fn resume(&self) {}
            fn dismiss(&self) {}
            fn dismiss_in_use(&self) {}
        }
        impl ModelGate for Quiet {
            fn ready(&self) -> bool {
                true
            }
            fn preparing(&self) -> Option<(f32, String)> {
                None
            }
            fn request_reload(&self) {}
        }
        impl WindowControl for Quiet {
            fn minimize(&self) {}
            fn toggle_maximize(&self) -> bool {
                false
            }
            fn hide(&self) {}
        }

        let d = tempfile::tempdir().unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/library");
        for f in std::fs::read_dir(&fixtures).unwrap().flatten() {
            std::fs::copy(f.path(), d.path().join(f.file_name())).unwrap();
        }
        let settings = Settings {
            output_dir: d.path().to_string_lossy().into_owned(),
            auto_mode: false,
            ..Settings::defaults(Profile::Dev)
        };
        let detector: DetectorFactory =
            Box::new(|_, _| Arc::new(Quiet) as Arc<dyn DetectorControl>);
        let app = App::new(
            settings,
            AppDeps {
                detector,
                sessions: Box::new(|_, _| Err("sin grabaciones en este test".into())),
                notifier: Arc::new(Quiet),
                model: Arc::new(Quiet),
                copy_text: Box::new(|_| Ok(())),
                events: Arc::new(|_| {}),
                save: Box::new(|_| Ok(())),
            },
        );
        let ui = Arc::new(UiApi::new(UiDeps {
            app,
            live_id: Box::new(|| None),
            copy: Box::new(|_| Ok(())),
            save_dialog: Box::new(|_| None),
            folder_dialog: Box::new(|| None),
            today: Box::new(|| chrono::NaiveDate::from_ymd_opt(2026, 11, 12).unwrap()),
            device_info: Box::new(|| "Procesador · small".into()),
            hotkey_capture: Box::new(|_| {}),
            window: Arc::new(Quiet),
            voices_path: d.path().join("voces.json"),
            prints_dir: d.path().join("huellas"),
            diagnostics: Box::new(String::new),
            version: "0.0.0-test".into(),
        }));
        let count = ui
            .list_recordings("all", "", None)
            .as_array()
            .unwrap()
            .len();
        assert_eq!(count, 5, "la carpeta de ejemplo");

        let (tx, replies) = mpsc::channel::<Reply>();
        let bridge = Rc::new(Bridge::new(
            ui,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        ));
        let seen: Rc<RefCell<Vec<Value>>> = Rc::default();
        let (b, log) = (bridge.clone(), seen.clone());
        let win = MainWindow::create(
            Box::new(move |raw| {
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    log.borrow_mut().push(v);
                }
                b.on_message(&raw);
            }),
            true,
            initial_rect(None),
        )
        .expect("ventana principal");
        let script = r##"(async () => {
            const until = async (f) => { for (let i = 0; i < 200 && !f(); i++) await new Promise((r) => setTimeout(r, 25)); };
            const texts = (sel) => [...document.querySelectorAll(sel)].map((e) => e.textContent.trim());
            await until(() => document.querySelectorAll(".item").length > 0);
            const items = document.querySelectorAll(".item").length;
            document.querySelector('.item[data-id="2026-11-12_1000_Teams.md"]').click();
            await until(() => document.querySelector(".ctitle")?.textContent === "Revisión de la web");
            window.ipc.postMessage(JSON.stringify({ id: 0, method: "visto", args: [{
                items,
                title: document.querySelector(".ctitle")?.textContent ?? null,
                client: document.querySelector("#client-btn")?.textContent.trim() ?? null,
                people: texts(".person[data-name]"),
                marks: texts(".line.marked .ts"),
            }] }));
        })();"##;
        let has = |method: &str| seen.borrow().iter().any(|v| v["method"] == method);
        let (end, mut sent) = (Instant::now() + Duration::from_secs(20), false);
        let mut msg = MSG::default();
        while Instant::now() < end && !has("visto") {
            // SAFETY: cola de mensajes de este hilo.
            unsafe {
                MsgWaitForMultipleObjects(None, false, 20, QS_ALLINPUT);
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            while let Ok(r) = replies.try_recv() {
                win.eval(&reply_js(r.id, &r.json));
            }
            if !sent && has("log_js") {
                win.eval(script);
                sent = true;
            }
        }
        let seen = seen.borrow();
        let visto = seen
            .iter()
            .find(|v| v["method"] == "visto")
            .unwrap_or_else(|| panic!("la página no respondió; llegó: {seen:?}"));
        assert_eq!(
            visto["args"][0],
            json!({
                "items": count,
                "title": "Revisión de la web",
                "client": "Acme",
                "people": ["María", "Persona 1"],
                "marks": ["00:00:05"],
            })
        );
    }

    type Seen = RefCell<Vec<u32>>;

    /// Mueve lo pendiente en este hilo (el "principal" del test) hasta que `done` o pasen 5 s.
    fn serve(calls: &MainCalls<Seen>, seen: &Seen, done: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !done() && Instant::now() < end {
            calls.run_pending(seen);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn test_main_thread_calls_run_in_order_and_return() {
        let calls: Arc<MainCalls<Seen>> = Arc::new(MainCalls::new(Box::new(|| {})));
        let seen = Seen::default();
        let c = calls.clone();
        let worker = thread::spawn(move || {
            let a = c.call(None, |s: &Seen| {
                s.borrow_mut().push(1);
                "uno"
            });
            c.post(|s: &Seen| s.borrow_mut().push(3));
            let b = c.call(Some(Duration::from_secs(5)), |s: &Seen| {
                s.borrow_mut().push(2);
                s.borrow().len()
            });
            (a, b)
        });
        serve(&calls, &seen, || worker.is_finished());
        assert_eq!(worker.join().unwrap(), (Some("uno"), Some(3)));
        assert_eq!(*seen.borrow(), [1, 3, 2], "lo publicado va en su turno");
    }

    /// Si el hilo principal no la atiende a tiempo, la llamada no se hace nunca (p. ej. soltar los
    /// atajos después de que la captura ya terminó).
    #[test]
    fn test_main_thread_call_times_out_and_never_runs() {
        let calls: Arc<MainCalls<Seen>> = Arc::new(MainCalls::new(Box::new(|| {})));
        let c = calls.clone();
        let got = thread::spawn(move || {
            c.call(Some(Duration::from_millis(50)), |s: &Seen| {
                s.borrow_mut().push(9)
            })
        })
        .join()
        .unwrap();
        assert_eq!(got, None);
        let seen = Seen::default();
        calls.run_pending(&seen);
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn test_closed_calls_return_none() {
        let calls: Arc<MainCalls<Seen>> = Arc::new(MainCalls::new(Box::new(|| {})));
        let c = calls.clone();
        let waiting = thread::spawn(move || c.call(None, |s: &Seen| s.borrow_mut().push(1)));
        let end = Instant::now() + Duration::from_secs(5);
        while calls.queue.lock().unwrap().is_empty() && Instant::now() < end {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!calls.queue.lock().unwrap().is_empty(), "en la cola");
        calls.close();
        assert_eq!(waiting.join().unwrap(), None, "la que esperaba");
        let c = calls.clone();
        let after = thread::spawn(move || c.call(None, |s: &Seen| s.borrow_mut().push(2)));
        assert_eq!(after.join().unwrap(), None, "las nuevas");
        let seen = Seen::default();
        calls.run_pending(&seen);
        assert!(seen.borrow().is_empty());
    }

    /// Desde el propio hilo principal se esperaría a sí mismo: no se hace.
    #[test]
    fn test_call_from_main_thread_is_refused() {
        let calls: MainCalls<Seen> = MainCalls::new(Box::new(|| {}));
        assert_eq!(calls.call(None, |_: &Seen| 1), None);
    }
}

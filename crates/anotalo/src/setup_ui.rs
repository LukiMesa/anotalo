//! La ventana del instalador y del desinstalador: `web/setup.html` en una ventana de tamaño fijo
//! (la de la app, `MainWindow`, con otra página), con los colores y las fuentes de la app.
//!
//! La página pide `setup_info` (qué hay instalado, qué opciones) y `setup_run` (instalar con
//! `{autostart, desktop}` o desinstalar con `{purge}`), que trabaja en otro hilo y cuenta sus pasos
//! con avisos `{"type": "setup", "step": …}`. Al instalar bien, la ventana se cierra sola (la app ya
//! se abrió); al desinstalar, la cierra el usuario con "Cerrar" (`setup_close`). Cerrar la ventana
//! mientras trabaja no la para: termina y entonces sale.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anotalo_setup::install::{Found, Step};
use serde_json::{Value, json};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostQuitMessage, WM_APP};

use crate::shell::main_window::{MainWindow, Spec, WindowChange, centered_rect};
use crate::shell::window::{self, Handler, WM_APP_EVENT, WM_APP_UI};
use crate::ui_bridge::{Bridge, Commands, Reply, event_js, reply_js, unknown_method};

/// 460×440 a 96 ppp, sin redimensionar.
pub const SIZE: (i32, i32) = (460, 440);
/// Cerrar la ventana (la página, la X, Alt+F4 o al terminar de instalar).
const WM_APP_SETUP_CLOSE: u32 = WM_APP + 9;

/// Lo que la página necesita saber: qué se hace, qué hay instalado y las opciones por defecto.
pub fn info(mode: &str, found: &Found, version: &str, recordings: &Path) -> Value {
    json!({
        "mode": mode,
        "version": version,
        "installed": found.anotalo,
        "transcriptor": found.inno.is_some(),
        "app_running": found.app_running,
        "recordings": recordings.display().to_string(),
        "options": {"autostart": found.defaults.autostart, "desktop": found.defaults.desktop},
    })
}

/// Los datos de WebView2 del instalador: en `%TEMP%`, aparte de los de la app, y se borran al
/// cerrar. Sin una carpeta propia, WebView2 crearía `Anotalo-Setup-x.exe.WebView2` en Descargas.
pub fn webview_data_dir() -> PathBuf {
    std::env::temp_dir().join("Anotalo-setup-webview")
}

/// El nombre de un paso para la página.
pub fn step_name(s: Step) -> &'static str {
    match s {
        Step::Closing => "closing",
        Step::Copying => "copying",
        Step::Shortcuts => "shortcuts",
        Step::Removing => "removing",
        Step::Launching => "launching",
        Step::Done => "done",
    }
}

/// Lo que hace el botón, con las opciones de la página. Cuenta los pasos por `step`. Ok trae el
/// exe que hay que borrar al salir (el desinstalador en su sitio).
pub type Runner = Box<
    dyn Fn(&Value, &mut dyn FnMut(&'static str)) -> Result<Option<PathBuf>, String> + Send + Sync,
>;

/// Cómo terminó: None si no se llegó a ejecutar.
pub type Outcome = Option<Result<Option<PathBuf>, String>>;

struct Inner {
    info: Value,
    runner: Runner,
    push: Box<dyn Fn(Value) + Send + Sync>,
    close: Box<dyn Fn() + Send + Sync>,
    /// Instalar: al terminar bien, la ventana se cierra sola.
    close_when_done: bool,
    busy: AtomicBool,
    /// Se pidió cerrar mientras trabajaba: se cierra al terminar.
    close_pending: AtomicBool,
    result: Mutex<Outcome>,
}

/// Las órdenes de la página del instalador.
pub struct SetupCommands(Arc<Inner>);

impl SetupCommands {
    pub fn new(
        info: Value,
        runner: Runner,
        push: Box<dyn Fn(Value) + Send + Sync>,
        close: Box<dyn Fn() + Send + Sync>,
        close_when_done: bool,
    ) -> SetupCommands {
        SetupCommands(Arc::new(Inner {
            info,
            runner,
            push,
            close,
            close_when_done,
            busy: AtomicBool::new(false),
            close_pending: AtomicBool::new(false),
            result: Mutex::new(None),
        }))
    }

    pub fn busy(&self) -> bool {
        self.0.busy.load(Ordering::SeqCst)
    }

    /// Cerrar cuando termine lo que está en marcha.
    pub fn close_when_idle(&self) {
        self.0.close_pending.store(true, Ordering::SeqCst);
        if !self.busy() {
            (self.0.close)();
        }
    }

    pub fn result(&self) -> Outcome {
        self.0
            .result
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Espera a que termine lo que está en marcha (las pruebas).
    #[cfg(test)]
    pub fn wait_idle(&self, limit: Duration) {
        let end = Instant::now() + limit;
        while self.busy() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn run(&self, opts: Value) -> Value {
        if self.0.busy.swap(true, Ordering::SeqCst) {
            return json!({"ok": false, "error": "Ya está en marcha"});
        }
        let inner = self.0.clone();
        let spawned = std::thread::Builder::new()
            .name("instalador".into())
            .spawn(move || {
                let r = (inner.runner)(&opts, &mut |step| {
                    if step != "done" {
                        (inner.push)(json!({"type": "setup", "step": step}));
                    }
                });
                let event = match &r {
                    Ok(_) => json!({"type": "setup", "step": "done"}),
                    Err(e) => {
                        log::error!("{e}");
                        json!({"type": "setup", "step": "error", "error": e})
                    }
                };
                let ok = r.is_ok();
                // antes del aviso: así "Reintentar" ya se puede pulsar
                *inner.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                inner.busy.store(false, Ordering::SeqCst);
                (inner.push)(event);
                if (ok && inner.close_when_done) || inner.close_pending.load(Ordering::SeqCst) {
                    (inner.close)();
                }
            });
        match spawned {
            Ok(_) => json!({"ok": true}),
            Err(e) => {
                self.0.busy.store(false, Ordering::SeqCst);
                json!({"ok": false, "error": format!("No se pudo empezar: {e}")})
            }
        }
    }
}

impl Commands for SetupCommands {
    fn call(&self, method: &str, args: &[Value]) -> Value {
        match method {
            "setup_info" => self.0.info.clone(),
            "setup_run" => self.run(args.first().cloned().unwrap_or(Value::Null)),
            "setup_close" if self.busy() => {
                json!({"ok": false, "error": "Espera a que termine"})
            }
            "setup_close" => {
                (self.0.close)();
                json!({"ok": true})
            }
            "log_js" => {
                log::debug!("Página del instalador: {args:?}");
                json!({"ok": true})
            }
            other => unknown_method(other),
        }
    }
}

/// La ventana en marcha, en el hilo principal.
struct SetupShell {
    window: RefCell<Option<MainWindow>>,
    replies: Receiver<Reply>,
    events: Receiver<Value>,
    commands: Arc<SetupCommands>,
}

impl SetupShell {
    fn eval(&self, js: &str) {
        if let Ok(w) = self.window.try_borrow()
            && let Some(w) = w.as_ref()
        {
            w.eval(js);
        }
    }
}

impl Handler for SetupShell {
    fn handle(&self, _hwnd: HWND, msg: u32, _wp: WPARAM, _lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_APP_UI => {
                while let Ok(r) = self.replies.try_recv() {
                    self.eval(&reply_js(r.id, &r.json));
                }
            }
            WM_APP_EVENT => {
                while let Ok(e) = self.events.try_recv() {
                    self.eval(&event_js(&e));
                }
            }
            WM_APP_SETUP_CLOSE => {
                if self.commands.busy() {
                    // termina en segundo plano y luego sale
                    self.commands.close_when_idle();
                } else {
                    // SAFETY: cola de este hilo.
                    unsafe { PostQuitMessage(0) };
                }
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

/// Abre la ventana y la atiende hasta que se cierre. Err si no se pudo abrir (sin WebView2 o si
/// falla): entonces se usan los cuadros de mensaje. `close_when_done`: instalar.
pub fn run_window(
    title: &'static str,
    info: Value,
    runner: Runner,
    close_when_done: bool,
) -> Result<Outcome, String> {
    let hwnd = window::create()?;
    let hwnd_raw = hwnd.0 as isize;
    let (event_tx, events) = mpsc::channel::<Value>();
    let commands = Arc::new(SetupCommands::new(
        info,
        runner,
        Box::new(move |e| {
            if event_tx.send(e).is_ok() {
                window::post(hwnd_raw, WM_APP_EVENT, 0);
            }
        }),
        Box::new(move || window::post(hwnd_raw, WM_APP_SETUP_CLOSE, 0)),
        close_when_done,
    ));
    let (reply_tx, replies) = mpsc::channel::<Reply>();
    let bridge = Rc::new(Bridge::new(
        commands.clone(),
        Box::new(move |r| {
            if reply_tx.send(r).is_ok() {
                window::post(hwnd_raw, WM_APP_UI, 0);
            }
        }),
    ));
    let data_dir = webview_data_dir();
    let win = MainWindow::create_with(
        Box::new(move |raw| bridge.on_message(&raw)),
        Spec {
            page: "setup.html",
            title,
            data_dir: data_dir.clone(),
            fixed: Some(SIZE),
            start_hidden: false,
            rect: centered_rect(SIZE),
        },
    )?;
    // la X o Alt+F4 la ocultan: es cerrar
    win.set_listener(Box::new(move |c| {
        if c == WindowChange::Hidden {
            window::post(hwnd_raw, WM_APP_SETUP_CLOSE, 0);
        }
    }));
    let shell = Rc::new(SetupShell {
        window: RefCell::new(Some(win)),
        replies,
        events,
        commands: commands.clone(),
    });
    window::set_handler(Some(shell.clone()));
    window::run();
    window::set_handler(None);
    shell.window.borrow_mut().take(); // cierra WebView2 y la ventana
    remove_webview_data(&data_dir);
    Ok(commands.result())
}

/// WebView2 tarda un momento en soltar su carpeta al cerrarse: se reintenta unos segundos.
fn remove_webview_data(dir: &Path) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) if Instant::now() >= end => {
                log::debug!("No se pudo borrar {} ({e})", dir.display());
                return;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anotalo_setup::install::{Found, Options};
    use serde_json::json;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn test_setup_info_shapes() {
        let found = Found {
            anotalo: Some("1.0.0-alpha.2".into()),
            inno: None,
            app_running: true,
            legacy_running: false,
            defaults: Options {
                autostart: true,
                desktop: false,
            },
        };
        let v = info(
            "install",
            &found,
            "1.0.0-alpha.3",
            Path::new(r"C:\Users\a\Documents\Llamadas"),
        );
        assert_eq!(
            v,
            json!({"mode": "install", "version": "1.0.0-alpha.3", "installed": "1.0.0-alpha.2",
            "transcriptor": false, "app_running": true, "recordings": r"C:\Users\a\Documents\Llamadas",
            "options": {"autostart": true, "desktop": false}})
        );
    }

    #[test]
    fn test_installer_webview_data_is_in_temp() {
        assert_eq!(
            webview_data_dir(),
            std::env::temp_dir().join("Anotalo-setup-webview")
        );
    }

    /// Las órdenes de la página, con un `Runner` de mentira: `setup_run` vuelve enseguida, los
    /// pasos llegan como avisos, no se puede lanzar dos veces ni cerrar mientras trabaja.
    #[test]
    fn test_setup_commands_run_once_and_report_steps() {
        let (events, got) = mpsc::channel::<serde_json::Value>();
        let (closes, closed) = mpsc::channel::<()>();
        let (go, gate) = mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate);
        let cmds = SetupCommands::new(
            json!({"mode": "install"}),
            Box::new(move |opts, step| {
                assert_eq!(opts["autostart"], json!(false));
                step("closing");
                step("copying");
                let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
                Err("disco lleno".into())
            }),
            Box::new(move |e| {
                let _ = events.send(e);
            }),
            Box::new(move || {
                let _ = closes.send(());
            }),
            true,
        );
        assert_eq!(cmds.call("setup_info", &[]), json!({"mode": "install"}));
        let opts = json!({"autostart": false, "desktop": true});
        assert_eq!(
            cmds.call("setup_run", std::slice::from_ref(&opts)),
            json!({"ok": true})
        );
        assert_eq!(
            cmds.call("setup_run", std::slice::from_ref(&opts))["ok"],
            json!(false)
        );
        assert_eq!(cmds.call("setup_close", &[])["ok"], json!(false));
        let step = |e: serde_json::Value| e["step"].as_str().unwrap().to_string();
        let first = got.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first["type"], json!("setup"));
        assert_eq!(step(first), "closing");
        assert_eq!(
            step(got.recv_timeout(Duration::from_secs(5)).unwrap()),
            "copying"
        );
        go.send(()).unwrap();
        let err = got.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            err,
            json!({"type": "setup", "step": "error", "error": "disco lleno"})
        );
        assert!(closed.try_recv().is_err(), "un error no cierra la ventana");
        assert_eq!(cmds.result(), Some(Err("disco lleno".into())));
        // se puede reintentar, y al terminar bien (instalar) la ventana se cierra sola
        let ok = SetupCommands::new(
            json!({}),
            Box::new(|_, step| {
                step("launching");
                Ok(None)
            }),
            Box::new(|_| {}),
            Box::new(move || {
                let _ = std::fs::metadata("."); // nada
            }),
            true,
        );
        assert_eq!(ok.call("setup_run", &[json!({})]), json!({"ok": true}));
        ok.wait_idle(Duration::from_secs(5));
        assert_eq!(ok.result(), Some(Ok(None)));
        assert_eq!(cmds.call("setup_close", &[]), json!({"ok": true}));
        assert!(closed.recv_timeout(Duration::from_secs(5)).is_ok());
        assert_eq!(cmds.call("no_existe", &[])["ok"], json!(false));
    }
}

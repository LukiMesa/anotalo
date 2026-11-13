//! La app de la bandeja (`anotalo.exe`): une la app (core) con Windows — detector de llamadas,
//! sesiones, modelo, atajos, bandeja, indicador, avisos y la ventana principal. Todo lo de las
//! ventanas va en el hilo principal; lo que puede tardar (parar una grabación, liberar el modelo,
//! las órdenes de la página) va en otros hilos, que piden al principal lo suyo con `MainCalls`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use anotalo_core::app::{
    App, AppDeps, AppEvent, Applier, DetectorFactory, EventSink, Recording, SessionFactory,
    SessionRequest, State,
};
use anotalo_core::applog;
use anotalo_core::config::{
    LoadProblem, Settings, config_path, load_settings_checked, migrate_legacy, save_settings,
};
use anotalo_core::detector::{
    CallDetector, DetectorControl, DetectorHandle, MicProbe, POLL, Probes, meeting_title,
};
use anotalo_core::engine::whisper_factory;
use anotalo_core::model::{ModelConfig, ModelGate, ModelManager, igpu_check_needed};
use anotalo_core::models::{HttpFetch, Placement, choose, models_dir};
use anotalo_core::notify::Notifier;
use anotalo_core::paths::{self, Profile};
use anotalo_core::routing::{Platform, site_from_title};
use anotalo_core::session::{
    CallSession, Hurry, OnLevel, OnLine, SessionConfig, SourceFactory, Speakers,
};
use anotalo_core::speaker::memory::{VoiceStore, prune_voice_backups, voices_path};
use anotalo_core::speaker::model::SpeakerModel;
use anotalo_core::speaker::naming::{KEEP_CALL_PRINTS, prune_call_prints};
use anotalo_core::ui_api::{UiApi, UiDeps, WindowControl};
use anotalo_core::vad::Vad;
use anotalo_whisper::{Device, DeviceKind};
use anotalo_winaudio::coreaudio::{self, Snapshot};
use anotalo_winaudio::sources::{pick_mic, pick_others};
use anotalo_winsys::{clipboard, consent, instance, toast};
use serde_json::{Value, json};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    ENDSESSION_CRITICAL, KillTimer, PostQuitMessage, SW_SHOWNORMAL, SetTimer, WM_COMMAND,
    WM_CONTEXTMENU, WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONUP, WM_QUERYENDSESSION, WM_RBUTTONUP,
    WM_TIMER,
};
use windows::core::{HSTRING, w};

use crate::diagnostics;
use crate::record::vad_factory;
use crate::shell::file_dialogs;
use crate::shell::hotkeys::{HotkeyId, Hotkeys};
use crate::shell::indicator::{Indicator, IndicatorWiring, Pill, shut_down};
use crate::shell::main_window::{MainWindow, WindowChange, initial_rect};
use crate::shell::tray::{Tray, TrayCommand, WM_APP_TRAY, menu_items};
use crate::shell::window::{
    self, Handler, WM_APP_CALL, WM_APP_EVENT, WM_APP_QUIT_DONE, WM_APP_SHOW, WM_APP_UI,
    WM_APP_WINDOW,
};
use crate::ui_bridge::{Bridge, Reply, event_js, reply_js};
use crate::ui_wiring::{
    MainCalls, NO_WINDOW_TITLE, PageCtx, check_hotkey, device_label, no_window_body, page_event,
};
use crate::voices_cli::prints_dir;

/// Cada 30 s se mira si el modelo lleva tiempo sin usarse (para liberarlo).
const MODEL_TIMER: usize = 1;
const MODEL_TICK_MS: u32 = 30_000;
/// Si el icono no entra en la bandeja (el Explorador aún ocupado al iniciar sesión), se reintenta
/// cada 5 s durante un minuto; después, solo cuando el Explorador avise de que arrancó.
const TRAY_TIMER: usize = 2;
const TRAY_RETRY_MS: u32 = 5_000;
const TRAY_RETRIES: u32 = 12;
/// Al salir con "Salir", lo que se espera a que se vean los avisos pendientes.
const TOASTS_WAIT: Duration = Duration::from_secs(3);
/// Lo que un hilo de las órdenes espera a que el hilo principal cambie un atajo o mueva la
/// ventana; si no llega a tiempo, se abandona (nunca se hace tarde).
const MAIN_WAIT: Duration = Duration::from_secs(5);

/// Las piezas se crean antes que la app (el detector y el modelo van dentro de ella): leen los
/// ajustes a través de esto, que apunta a la app en cuanto existe.
#[derive(Clone)]
struct AppRef {
    cell: Arc<OnceLock<Weak<App>>>,
    initial: Arc<Settings>,
}

impl AppRef {
    fn app(&self) -> Option<Arc<App>> {
        self.cell.get().and_then(Weak::upgrade)
    }

    fn settings(&self) -> Settings {
        self.app()
            .map_or_else(|| (*self.initial).clone(), |a| a.settings())
    }
}

/// Muestra un aviso: título, texto y el archivo que abre al pulsarlo.
type ShowToast = Arc<dyn Fn(&str, &str, Option<&Path>) -> Result<(), String> + Send + Sync>;

/// Cuántos avisos se están mostrando aún.
type Counter = Arc<(Mutex<usize>, Condvar)>;

/// Avisos del sistema, en otro hilo (WinRT tarda un poco y no debe frenar a nadie). Al salir se
/// espera un poco a los que aún se están mostrando ("Transcripción guardada" tras "Salir").
struct Toasts {
    show: ShowToast,
    showing: Counter,
}

/// Un aviso en curso: cuenta mientras vive (se descuenta también si el aviso falla).
struct Showing(Counter);

impl Showing {
    fn new(c: &Counter) -> Showing {
        *c.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Showing(c.clone())
    }
}

impl Drop for Showing {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.0.1.notify_all();
    }
}

impl Toasts {
    fn new() -> Toasts {
        Toasts::with(Arc::new(|t, b, o| toast::show(toast::AUMID, t, b, o)))
    }

    fn with(show: ShowToast) -> Toasts {
        Toasts {
            show,
            showing: Arc::default(),
        }
    }

    /// Espera, como mucho `timeout`, a que terminen de mostrarse los avisos. false si no dio tiempo.
    fn wait(&self, timeout: Duration) -> bool {
        let (n, done) = &*self.showing;
        let n = n.lock().unwrap_or_else(|e| e.into_inner());
        let (n, _) = done
            .wait_timeout_while(n, timeout, |n| *n > 0)
            .unwrap_or_else(|e| e.into_inner());
        *n == 0
    }
}

impl Notifier for Toasts {
    fn notify(&self, title: &str, body: &str, open: Option<&Path>) {
        log::info!("Aviso: {title} — {body}");
        let (t, b, o) = (
            title.to_string(),
            body.to_string(),
            open.map(Path::to_path_buf),
        );
        let (show, showing) = (self.show.clone(), Showing::new(&self.showing));
        let _ = std::thread::Builder::new()
            .name("aviso".into())
            .spawn(move || {
                let _showing = showing;
                if let Err(e) = show(&t, &b, o.as_deref()) {
                    log::warn!("No se pudo mostrar el aviso: {e}");
                }
            });
    }
}

/// Para no repetir una acción por un doble clic (que son dos clics).
struct Debounce {
    gap: Duration,
    last: Cell<Option<Instant>>,
}

impl Debounce {
    fn new(gap: Duration) -> Debounce {
        Debounce {
            gap,
            last: Cell::new(None),
        }
    }

    /// true si la acción va: la anterior fue hace `gap` o más.
    fn ready(&self, now: Instant) -> bool {
        if self
            .last
            .get()
            .is_some_and(|l| now.saturating_duration_since(l) < self.gap)
        {
            return false;
        }
        self.last.set(Some(now));
        true
    }
}

/// Una grabación de verdad.
struct Live(CallSession);

impl Recording for Live {
    fn stop(self: Box<Self>, hurry: &Hurry) -> io::Result<(PathBuf, String)> {
        self.0.stop_with_text(hurry)
    }

    fn mark(&self) -> Option<f64> {
        self.0.mark()
    }

    fn path(&self) -> Option<PathBuf> {
        Some(self.0.path().to_path_buf())
    }
}

/// Para una llamada: el modelo de voces si ya está listo (si no, se graba sin separar y se
/// sigue preparando para la siguiente).
fn speakers_for_call(voices: &Arc<SpeakerModel>, profile: Profile) -> Option<Speakers> {
    voices.ensure();
    match voices.get() {
        Some(embed) => Some(Speakers {
            embed,
            memory: VoiceStore::load(&voices_path(profile)).voices,
            prints_dir: prints_dir(profile),
        }),
        None => {
            log::info!("Separación de voces aún no disponible: esta llamada se graba sin separar");
            None
        }
    }
}

fn sessions(
    r: AppRef,
    model: Arc<ModelManager>,
    voices: Arc<SpeakerModel>,
    profile: Profile,
) -> SessionFactory {
    Box::new(move |req: SessionRequest, events: EventSink| {
        let s = r.settings();
        let transcriber = model.acquire()?;
        // las notas rápidas son solo el micrófono: no hay "Otros" que separar
        let speakers = req
            .others
            .as_ref()
            .and_then(|_| speakers_for_call(&voices, profile));
        let platform = req.others.as_ref().and_then(|o| o.platform.clone());
        let mic = pick_mic(platform.as_deref(), s.mic_device.as_deref());
        let (others, again) = match req.others {
            None => (None, None),
            Some(spec) => {
                let first = pick_others(&spec);
                let again: SourceFactory = Box::new(move || pick_others(&spec));
                (Some(first), Some(again))
            }
        };
        let ev = events.clone();
        let on_line: OnLine = Box::new(move |speaker, hms, start_s, text| {
            ev(AppEvent::Line {
                speaker: speaker.into(),
                hms: hms.into(),
                start_s,
                text: text.into(),
            });
        });
        let on_level: OnLevel = Box::new(move |v| events(AppEvent::Level(v)));
        let dir = PathBuf::from(&s.output_dir);
        let cfg = SessionConfig {
            output_dir: dir.clone(),
            app_label: req.label,
            label_me: s.label_me,
            label_others: s.label_others,
            silence_ms: s.silence_ms,
            max_segment_s: s.max_segment_s,
            min_segment_ms: s.min_segment_ms,
            title: req.title,
            speakers,
        };
        CallSession::start(
            cfg,
            transcriber,
            mic,
            others,
            again,
            Some(on_line),
            Some(on_level),
            vad_factory(),
        )
        .map(|s| Box::new(Live(s)) as Box<dyn Recording>)
        .map_err(|e| {
            format!(
                "No se pudo crear la transcripción en {}: {e}",
                dir.display()
            )
        })
    })
}

fn active_keys(s: &Snapshot) -> HashSet<String> {
    s.sessions
        .iter()
        .filter(|x| x.active)
        .filter_map(|x| x.key.clone())
        .collect()
}

/// Quién tiene sonido abierto, para app.log (spec §4.6).
fn describe(s: &Result<Snapshot, String>) -> String {
    match s {
        Err(e) => format!("(no se pudo leer: {e})"),
        Ok(s) if s.sessions.is_empty() => "nadie".into(),
        Ok(s) => s
            .sessions
            .iter()
            .map(|x| {
                let who = x.key.clone().unwrap_or_else(|| format!("pid {}", x.pid));
                let on = if x.active { ", activa" } else { "" };
                format!("{who} ({}{on})", x.endpoint_name)
            })
            .collect::<Vec<_>>()
            .join("; "),
    }
}

/// Cuando cambia quién usa el micro: las sesiones de grabación y de sonido, y lo que el registro
/// del micrófono (ConsentStore) marca en uso.
/// Las apps que el registro del micrófono marca en uso ahora.
fn consent_in_use(registry: &Result<HashMap<String, (u64, u64)>, String>) -> String {
    match registry {
        Ok(m) => {
            let mut keys: Vec<&str> = m
                .iter()
                .filter(|(_, (start, stop))| *start > 0 && *stop == 0)
                .map(|(k, _)| k.as_str())
                .collect();
            keys.sort_unstable();
            format!("{keys:?}")
        }
        Err(e) => format!("(no se pudo leer: {e})"),
    }
}

fn log_audio(
    registry: &Result<HashMap<String, (u64, u64)>, String>,
    capture: &Result<Snapshot, String>,
) {
    log::info!("ConsentStore en uso: {}", consent_in_use(registry));
    log::info!("Grabando: {}", describe(capture));
    log::info!("Sonando: {}", describe(&coreaudio::render_snapshot()));
}

/// El audio de ahora para "Copiar diagnóstico" (como lo anota app.log).
fn live_audio() -> diagnostics::Audio {
    diagnostics::Audio {
        render: describe(&coreaudio::render_snapshot()),
        capture: describe(&coreaudio::capture_snapshot()),
        consent: consent_in_use(&consent::mic_entries()),
    }
}

/// Los títulos que dicen algo de la llamada (el sitio o la reunión) y los demás: en un navegador,
/// la pestaña de cada una de sus ventanas (asuntos de correos…), que no van a app.log (M3 permite
/// copiarlo) salvo con el registro detallado.
fn split_titles<'a>(p: &Platform, titles: &'a [String]) -> (Vec<&'a str>, Vec<&'a str>) {
    titles.iter().map(String::as_str).partition(|t| {
        site_from_title(t).is_some() || meeting_title(p, &[(*t).to_string()]).is_some()
    })
}

fn detector(r: AppRef) -> DetectorFactory {
    Box::new(move |on_start, on_stop| {
        let probe = MicProbe::default();
        let last = Mutex::new(HashSet::new());
        let mic = Box::new(move || {
            let registry = consent::mic_entries();
            let capture = coreaudio::capture_snapshot();
            let keys = probe.read(
                registry.clone(),
                capture.as_ref().map(active_keys).map_err(Clone::clone),
            );
            let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
            if *last != keys {
                log_audio(&registry, &capture);
                *last = keys.clone();
            }
            keys
        });
        // solo al detectar una llamada; los títulos sirven para casos de prueba de `meeting_title`
        let windows = Box::new(|p: &'static Platform| {
            let titles = anotalo_winsys::windows::visible_titles(p.exes);
            let (useful, rest) = split_titles(p, &titles);
            log::info!("Ventanas de {}: {useful:?} y {} más", p.name, rest.len());
            if !rest.is_empty() {
                log::debug!("Las demás ventanas de {}: {rest:?}", p.name);
            }
            titles
        });
        let enabled = Box::new(move |p: &Platform| r.settings().detect_enabled(p.setting));
        let t0 = Instant::now();
        let clock = Box::new(move || t0.elapsed().as_secs_f64());
        let d = CallDetector::new(on_start, on_stop, Probes { mic, windows }, enabled, clock);
        DetectorHandle::new(d, POLL) as Arc<dyn DetectorControl>
    })
}

fn model_manager(
    r: &AppRef,
    notifier: Arc<dyn Notifier>,
    devices: Arc<Vec<Device>>,
) -> Arc<ModelManager> {
    let release_after = Arc::new(AtomicU64::new(u64::from(r.initial.release_after_min) * 60));
    let (rf, rp, rc, rs, ro) = (r.clone(), r.clone(), r.clone(), r.clone(), r.clone());
    let (dp, dc) = (devices.clone(), devices);
    let cfg = ModelConfig {
        factory: Box::new(move |pick, progress| {
            let s = rf.settings();
            whisper_factory(
                pick.map_or(s.model, String::from),
                s.language,
                models_dir(),
                Arc::new(HttpFetch::default()),
                progress,
            )
        }),
        placement: Box::new(move || {
            let s = rp.settings();
            choose(&s.device, &dp, s.igpu_faster)
        }),
        release_after,
        igpu_check: Box::new(move || igpu_check_needed(&rc.settings(), &dc)),
        save_igpu: Box::new(move |faster, name| {
            if let Some(app) = rs.app() {
                for (k, v) in [
                    ("igpu_faster", Value::Bool(faster)),
                    ("igpu_name", Value::String(name.clone())),
                ] {
                    if let Err(e) = app.apply_setting(k, v) {
                        log::warn!("No se pudo guardar {k}: {e}");
                    }
                }
            }
        }),
    };
    ModelManager::new(
        cfg,
        notifier,
        Box::new(move |_| {
            if let Some(app) = ro.app() {
                app.model_changed();
            }
        }),
    )
}

fn in_background(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.into()).spawn(f);
}

/// Borra las huellas de voz que ya no se guardan: las de llamadas de hace más de 30 días y las
/// copias y restos de guardados que quedan junto a ellas. Al arrancar y al terminar cada llamada
/// (la bandeja puede pasar semanas abierta).
fn prune_prints(profile: Profile) {
    let n = prune_call_prints(&prints_dir(profile), KEEP_CALL_PRINTS)
        + prune_voice_backups(&voices_path(profile), KEEP_CALL_PRINTS);
    if n > 0 {
        log::info!("Borrados {n} archivos de huellas de voz antiguos");
    }
}

fn open_folder(dir: &str) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!("No se pudo crear {dir}: {e}");
    }
    // SAFETY: cadenas válidas durante la llamada.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(dir),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
}

/// Dónde corre el modelo, para la ventana (como `device_info` de la app de Python).
fn device_text(model: &ModelManager, devices: &[Device], s: &Settings) -> String {
    let gpus: Vec<&Device> = devices
        .iter()
        .filter(|d| matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu))
        .collect();
    let chosen = choose(&s.device, devices, s.igpu_faster);
    let loaded = model.placement();
    let name = match loaded.unwrap_or(chosen) {
        Placement::Gpu { device } | Placement::IntegratedGpu { device } => usize::try_from(device)
            .ok()
            .and_then(|i| gpus.get(i))
            .map(|d| {
                if d.description.is_empty() {
                    d.name.as_str()
                } else {
                    d.description.as_str()
                }
            }),
        Placement::Cpu => None,
    };
    device_label(&model.status(), loaded, chosen, name, &s.model)
}

/// El nombre del archivo que se está grabando (el id de la página).
fn live_id(app: &App) -> Option<String> {
    app.live_path()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// Guarda dónde está la ventana (si cambió).
fn save_rect(app: &App, r: RECT) {
    let rect = [r.left, r.top, r.right, r.bottom];
    if app.settings().window_rect == Some(rect) {
        return;
    }
    if let Err(e) = app.apply_setting("window_rect", json!(rect)) {
        log::warn!("No se pudo guardar el sitio de la ventana: {e}");
    }
}

/// Los botones de la barra de la página, que mueven la ventana en el hilo principal.
struct WindowCalls(Arc<MainCalls<Shell>>);

impl WindowControl for WindowCalls {
    fn minimize(&self) {
        self.0.call(Some(MAIN_WAIT), |sh: &Shell| {
            sh.with_window(MainWindow::minimize)
        });
    }

    fn toggle_maximize(&self) -> bool {
        self.0
            .call(Some(MAIN_WAIT), |sh: &Shell| {
                sh.with_window(MainWindow::toggle_maximize)
            })
            .flatten()
            .unwrap_or(false)
    }

    fn hide(&self) {
        self.0.call(Some(MAIN_WAIT), |sh: &Shell| {
            sh.with_window(MainWindow::hide)
        });
    }
}

/// Cambiar un atajo desde la ventana: válido, distinto del otro atajo y libre (se registra en el
/// hilo principal, que es el dueño de los atajos).
fn hotkey_applier(calls: Arc<MainCalls<Shell>>, r: AppRef, id: HotkeyId) -> Applier {
    Box::new(move |v| {
        let spec = v
            .as_str()
            .ok_or_else(|| "Valor no válido".to_string())?
            .to_string();
        let s = r.settings();
        let (other, name) = match id {
            HotkeyId::Record => (s.hotkey_mark, "marcar"),
            HotkeyId::Mark => (s.hotkey, "grabar"),
        };
        check_hotkey(&spec, &other, name)?;
        calls
            .call(Some(MAIN_WAIT), move |sh: &Shell| sh.rebind(id, &spec))
            .unwrap_or_else(|| Err("No se pudo cambiar el atajo ahora; inténtalo otra vez".into()))
    })
}

/// Lo que vive en el hilo principal y atiende la ventana.
struct Shell {
    hwnd: isize,
    app: Arc<App>,
    model: Arc<ModelManager>,
    rx: Receiver<AppEvent>,
    tray: RefCell<Option<Tray>>,
    hotkeys: RefCell<Option<Hotkeys>>,
    indicator: RefCell<Option<Arc<Indicator>>>,
    wiring: Arc<IndicatorWiring>,
    /// La ventana principal (None si WebView2 no está o falló).
    window: RefCell<Option<MainWindow>>,
    ui: Arc<UiApi>,
    /// Respuestas de las órdenes de la página, para entregarlas en este hilo.
    replies: Receiver<Reply>,
    calls: Arc<MainCalls<Shell>>,
    devices: Arc<Vec<Device>>,
    /// Lo último que muestra la bandeja (no se rehace el icono si no cambia).
    shown: Cell<Option<(State, Option<u8>)>>,
    /// Intentos fallidos seguidos de poner el icono.
    tray_failures: Cell<u32>,
    /// El clic en el icono (un doble clic abre la carpeta una sola vez).
    clicks: Debounce,
    toasts: Arc<Toasts>,
    quitting: Cell<bool>,
    taskbar_created: u32,
    profile: Profile,
}

impl Shell {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    fn drain(&self) {
        // lo que cuentan los eventos de la página, una vez por tanda
        let mut ctx: Option<PageCtx> = None;
        while let Ok(e) = self.rx.try_recv() {
            self.wiring.on_event(&e);
            if self.has_window() {
                let c = ctx.get_or_insert_with(|| self.page_ctx());
                if let Some(v) = page_event(&e, c) {
                    self.page(&v);
                }
            }
            match &e {
                AppEvent::State(s) => self.show_state(*s),
                AppEvent::SessionEnded { .. } => {
                    self.model.touch(Instant::now());
                    let p = self.profile;
                    in_background("borrar huellas", move || prune_prints(p));
                }
                AppEvent::Line {
                    speaker, hms, text, ..
                } => log::debug!("[{hms}] {speaker}: {text}"),
                _ => {}
            }
        }
    }

    fn page_ctx(&self) -> PageCtx {
        let s = self.app.settings();
        PageCtx {
            live_id: live_id(&self.app),
            device: device_text(&self.model, &self.devices, &s),
            label_me: s.label_me,
            hotkey: s.hotkey,
            preparing: self.model.preparing(),
            kind: self.app.session_kind(),
        }
    }

    fn has_window(&self) -> bool {
        self.window.try_borrow().is_ok_and(|w| w.is_some())
    }

    fn with_window<R>(&self, f: impl FnOnce(&MainWindow) -> R) -> Option<R> {
        self.window.try_borrow().ok()?.as_ref().map(f)
    }

    /// Un evento para la página (`window.onEvent`).
    fn page(&self, event: &Value) {
        self.with_window(|w| w.eval(&event_js(event)));
    }

    /// La ventana dueña de los diálogos.
    fn owner(&self) -> Option<HWND> {
        self.with_window(MainWindow::hwnd)
    }

    /// Muestra la ventana; false si no hay.
    fn show_window(&self) -> bool {
        self.with_window(MainWindow::show).is_some()
    }

    fn save_window_rect(&self) {
        if let Some(r) = self.with_window(MainWindow::rect) {
            let app = self.app.clone();
            in_background("guardar ventana", move || save_rect(&app, r));
        }
    }

    fn rebind(&self, id: HotkeyId, spec: &str) -> Result<(), String> {
        match self.hotkeys.try_borrow_mut() {
            Ok(mut h) => match h.as_mut() {
                Some(h) => h.rebind(id, spec),
                None => Err("La app se está cerrando".into()),
            },
            Err(_) => Err("No se pudo cambiar el atajo ahora; inténtalo otra vez".into()),
        }
    }

    /// Suelta (true) o devuelve (false) los atajos mientras la ventana captura uno nuevo.
    fn pause_hotkeys(&self, on: bool) {
        if let Ok(mut h) = self.hotkeys.try_borrow_mut()
            && let Some(h) = h.as_mut()
        {
            if on {
                h.pause();
            } else {
                h.resume();
            }
        }
    }

    /// Crea la ventana principal. Sin WebView2 (o si falla) se avisa y la app sigue en la bandeja.
    fn open_window(&self, start_hidden: bool, bridge: Rc<Bridge>, saved: Option<[i32; 4]>) {
        let rect = initial_rect(saved.map(|[left, top, right, bottom]| RECT {
            left,
            top,
            right,
            bottom,
        }));
        let made = if std::env::var_os("ANOTALO_FAKE_WEBVIEW_FAILURE").is_some_and(|v| v == "1") {
            Err("fallo simulado (ANOTALO_FAKE_WEBVIEW_FAILURE)".to_string())
        } else {
            MainWindow::create(
                Box::new(move |raw| bridge.on_message(&raw)),
                start_hidden,
                rect,
            )
        };
        match made {
            Ok(w) => {
                let hwnd = self.hwnd;
                w.set_listener(Box::new(move |c| {
                    let code = match c {
                        WindowChange::Hidden => 0,
                        WindowChange::Maximized(true) => 1,
                        WindowChange::Maximized(false) => 2,
                    };
                    window::post(hwnd, WM_APP_WINDOW, code);
                }));
                log::info!(
                    "Ventana principal lista{}",
                    if start_hidden { " (oculta)" } else { "" }
                );
                *self.window.borrow_mut() = Some(w);
            }
            Err(e) => {
                log::error!("No se pudo abrir la ventana: {e}");
                self.toasts.notify(NO_WINDOW_TITLE, &no_window_body(), None);
            }
        }
    }

    fn show_state(&self, s: State) {
        // el porcentaje solo mientras se descarga; cargar del disco no tiene progreso
        let pct = self
            .model
            .preparing()
            .filter(|(_, text)| s == State::Preparing && text.starts_with("Descargando"))
            .map(|(p, _)| (p * 100.0) as u8);
        if self.shown.get() == Some((s, pct)) {
            return;
        }
        if let Some(t) = self.tray.borrow().as_ref() {
            t.update(s, pct);
            self.shown.set(Some((s, pct)));
        }
    }

    fn add_tray(&self) {
        let Ok(mut slot) = self.tray.try_borrow_mut() else {
            return;
        };
        *slot = None; // quita el anterior, si lo había
        match Tray::new(self.hwnd()) {
            Ok(t) => {
                log::info!("Icono en la bandeja");
                *slot = Some(t);
                self.tray_failures.set(0);
                // SAFETY: temporizador propio (si no estaba, no pasa nada).
                let _ = unsafe { KillTimer(Some(self.hwnd()), TRAY_TIMER) };
            }
            Err(e) => {
                // sin icono no se llega a "Salir": se reintenta un rato, no solo con el Explorador
                let n = self.tray_failures.get() + 1;
                self.tray_failures.set(n);
                if n <= TRAY_RETRIES {
                    log::warn!("{e}; se reintenta en {} s", TRAY_RETRY_MS / 1000);
                    // SAFETY: temporizador de la ventana propia.
                    unsafe { SetTimer(Some(self.hwnd()), TRAY_TIMER, TRAY_RETRY_MS, None) };
                } else {
                    log::warn!("{e}; se reintenta cuando arranque el Explorador");
                    // SAFETY: temporizador propio.
                    let _ = unsafe { KillTimer(Some(self.hwnd()), TRAY_TIMER) };
                }
            }
        }
        drop(slot);
        self.shown.set(None);
        self.show_state(self.app.state());
    }

    /// Quita el "Guardando la transcripción…" de la pantalla de apagado, si estaba.
    fn unblock_shutdown(&self) {
        // SAFETY: ventana propia, de este hilo; si no había motivo, falla sin más.
        let _ = unsafe { ShutdownBlockReasonDestroy(self.hwnd()) };
    }

    fn menu(&self) {
        let items = menu_items(self.app.state(), self.app.settings().auto_mode);
        let cmd = self
            .tray
            .borrow()
            .as_ref()
            .and_then(|t| t.show_menu(&items));
        if let Some(c) = cmd {
            self.command(c);
        }
    }

    fn command(&self, c: TrayCommand) {
        let app = self.app.clone();
        match c {
            TrayCommand::ToggleNote => in_background("nota", move || app.toggle()),
            TrayCommand::RecordCall => in_background("llamada", move || app.toggle_call()),
            TrayCommand::ToggleAuto => in_background("modo automático", move || {
                let on = !app.settings().auto_mode;
                if let Err(e) = app.apply_setting("auto_mode", Value::Bool(on)) {
                    log::warn!("No se pudo cambiar el modo automático: {e}");
                }
            }),
            TrayCommand::OpenFolder => open_folder(&app.settings().output_dir),
            TrayCommand::Quit if !self.quitting.replace(true) => {
                log::info!("Cerrando Anotalo");
                let hwnd = self.hwnd;
                let rect = self.with_window(MainWindow::rect);
                in_background("salir", move || {
                    if let Some(r) = rect {
                        save_rect(&app, r);
                    }
                    app.quit();
                    window::post(hwnd, WM_APP_QUIT_DONE, 0);
                });
            }
            TrayCommand::Quit => {}
        }
    }

    /// Quita el icono, los atajos, el indicador y el motivo de bloqueo del apagado.
    fn teardown(&self) {
        // SAFETY: temporizadores propios.
        unsafe {
            let _ = KillTimer(Some(self.hwnd()), MODEL_TIMER);
            let _ = KillTimer(Some(self.hwnd()), TRAY_TIMER);
        }
        self.unblock_shutdown();
        // quien espera al hilo principal recibe None: ya no se le atiende
        self.calls.close();
        if let Ok(mut w) = self.window.try_borrow_mut() {
            w.take(); // cierra WebView2 y la ventana
        }
        if let Ok(mut h) = self.hotkeys.try_borrow_mut() {
            h.take();
        }
        if let Ok(mut t) = self.tray.try_borrow_mut() {
            t.take();
        }
        if let Some(ind) = self.indicator.borrow_mut().take() {
            shut_down(ind, &self.wiring);
        }
    }
}

impl Handler for Shell {
    fn handle(&self, _hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_APP_EVENT => self.drain(),
            WM_APP_UI => {
                while let Ok(r) = self.replies.try_recv() {
                    self.with_window(|w| w.eval(&reply_js(r.id, &r.json)));
                }
            }
            WM_APP_CALL => self.calls.run_pending(self),
            WM_APP_SHOW => {
                if !self.show_window() {
                    self.toasts.notify(
                        "Anotalo ya está abierto",
                        "Lo tienes en la bandeja del sistema, junto al reloj",
                        None,
                    );
                }
            }
            WM_APP_WINDOW => match wp.0 {
                0 => {
                    // se ocultó: una captura de atajo a medias termina (sin esperarla aquí)
                    let ui = self.ui.clone();
                    in_background("ventana oculta", move || ui.window_hidden());
                    if !self.quitting.get() {
                        self.save_window_rect();
                    }
                }
                code => self.page(&json!({"type": "window", "maximized": code == 1})),
            },
            WM_APP_TRAY => match (lp.0 as u32) & 0xFFFF {
                WM_RBUTTONUP | WM_CONTEXTMENU => self.menu(),
                // el clic muestra la ventana; sin ventana, abre las grabaciones (el doble clic, una
                // sola vez)
                WM_LBUTTONUP if self.clicks.ready(Instant::now()) && !self.show_window() => {
                    self.command(TrayCommand::OpenFolder);
                }
                _ => {}
            },
            WM_COMMAND => {
                if let Some(c) = TrayCommand::from_id((wp.0 & 0xFFFF) as i32) {
                    self.command(c);
                }
            }
            WM_HOTKEY if wp.0 == HotkeyId::Record as usize => {
                let app = self.app.clone();
                in_background("atajo", move || app.toggle());
            }
            WM_HOTKEY if wp.0 == HotkeyId::Mark as usize => {
                self.app.mark();
            }
            WM_TIMER if wp.0 == MODEL_TIMER => {
                let model = self.model.clone();
                in_background("liberar modelo", move || model.tick(Instant::now()));
            }
            WM_TIMER if wp.0 == TRAY_TIMER => {
                if self.tray.borrow().is_none() {
                    self.add_tray();
                } else {
                    // SAFETY: temporizador propio.
                    let _ = unsafe { KillTimer(Some(self.hwnd()), TRAY_TIMER) };
                }
            }
            WM_QUERYENDSESSION => {
                if self.app.recording_active() {
                    // SAFETY: ventana propia; el texto se copia.
                    let _ = unsafe {
                        ShutdownBlockReasonCreate(self.hwnd(), w!("Guardando la transcripción…"))
                    };
                }
                return Some(LRESULT(1));
            }
            // el apagado se canceló (otra app lo paró): se sigue como si nada
            WM_ENDSESSION if wp.0 == 0 => self.unblock_shutdown(),
            // Windows cierra el proceso al volver de aquí: la grabación se guarda antes. Si no lo
            // cierra (el gestor de reinicios de un instalador), la app sale sola: sin bandeja ni
            // atajos no debe quedarse con la instancia única.
            WM_ENDSESSION => {
                self.quitting.set(true);
                let rect = self.with_window(MainWindow::rect);
                if (lp.0 as u32) & ENDSESSION_CRITICAL != 0 {
                    // hay segundos: mejor perder las últimas frases que el final del archivo
                    log::info!("Windows se cierra a la fuerza: se guarda lo que haya, ya");
                    self.app.quit_fast();
                } else {
                    log::info!("Windows se cierra: se guarda lo que haya");
                    self.app.quit();
                }
                if let Some(r) = rect {
                    save_rect(&self.app, r);
                }
                self.teardown();
                // SAFETY: cola de este hilo.
                unsafe { PostQuitMessage(0) };
            }
            WM_APP_QUIT_DONE => {
                self.teardown();
                // que se llegue a ver "Transcripción guardada"
                if !self.toasts.wait(TOASTS_WAIT) {
                    log::info!("Se sale sin esperar más a los avisos");
                }
                // SAFETY: cola de este hilo.
                unsafe { PostQuitMessage(0) };
            }
            m if m == self.taskbar_created && m != 0 => {
                self.tray_failures.set(0);
                self.add_tray();
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

/// Sin consola propia (doble clic, inicio de Windows) se suelta la que Windows abrió para el exe.
/// Devuelve true si se lanzó desde una terminal (entonces el registro también sale por ella).
fn detach_console() -> bool {
    let mut ids = [0u32; 2];
    // SAFETY: búfer propio.
    let n = unsafe { GetConsoleProcessList(&mut ids) };
    if n == 1 {
        // SAFETY: soltar la consola propia.
        let _ = unsafe { FreeConsole() };
    }
    n > 1
}

pub fn run() -> ExitCode {
    let from_terminal = detach_console();
    let profile = paths::profile();
    let data = paths::data_dir(profile);
    if let Err(e) = std::fs::create_dir_all(&data).and_then(|()| applog::init(&data, from_terminal))
    {
        eprintln!("No se pudo abrir el registro en {}: {e}", data.display());
    }
    std::panic::set_hook(Box::new(|info| log::error!("Fallo interno: {info}")));
    log::info!("Anotalo {} ({profile:?})", env!("CARGO_PKG_VERSION"));

    let (mutex, show_event) = paths::instance_names(profile);
    let Some(_instance) = instance::acquire(mutex) else {
        log::info!("Ya hay una Anotalo abierta: se le avisa y se sale");
        instance::signal_show(show_event);
        return ExitCode::SUCCESS;
    };
    match migrate_legacy(profile) {
        Ok(true) => log::info!("Ajustes copiados de Transcriptor"),
        Ok(false) => {}
        Err(e) => log::warn!("No se pudieron copiar los ajustes de Transcriptor: {e}"),
    }
    let cfg_path = config_path(profile);
    let (settings, problem) = load_settings_checked(&cfg_path, profile);
    if let Err(e) = toast::register_aumid(toast::AUMID, "Anotalo")
        .and_then(|()| toast::set_process_aumid(toast::AUMID))
    {
        log::warn!("Los avisos pueden no verse: {e}");
    }
    let toasts = Arc::new(Toasts::new());
    let notifier: Arc<dyn Notifier> = toasts.clone();
    // un archivo que no se pudo leer no se sobrescribe con los valores por defecto
    let can_save = !matches!(problem, Some(LoadProblem::Unreadable(_)));
    match &problem {
        None => {}
        Some(LoadProblem::Damaged { backup: Some(b) }) => notifier.notify(
            "Ajustes dañados",
            &format!(
                "Uso los de por defecto. Tu archivo está copiado en {}",
                b.display()
            ),
            None,
        ),
        Some(LoadProblem::Damaged { backup: None }) => notifier.notify(
            "Ajustes dañados",
            "Uso los de por defecto. Revisa app.log",
            None,
        ),
        Some(LoadProblem::Unreadable(_)) => notifier.notify(
            "No se pudieron leer los ajustes",
            "Uso los de por defecto y no guardaré cambios hasta reiniciar. Revisa app.log",
            None,
        ),
    }

    let missing = anotalo_whisper::missing_cpu_features();
    if !missing.is_empty() {
        let e = anotalo_whisper::WhisperError::UnsupportedCpu(missing).to_string();
        log::error!("{e}");
        notifier.notify("Error", &e, None);
        std::thread::sleep(std::time::Duration::from_secs(2)); // que el aviso llegue a salir
        return ExitCode::from(1);
    }
    if let Err(e) = Vad::new() {
        log::error!("No se pudo cargar el detector de voz: {e}");
        return ExitCode::from(1);
    }
    let hwnd = match window::create() {
        Ok(h) => h,
        Err(e) => {
            log::error!("No se pudo crear la ventana de la app: {e}");
            return ExitCode::from(1);
        }
    };
    let hwnd_raw = hwnd.0 as isize;
    let start_hidden = std::env::args().nth(1).as_deref() == Some("--minimized");

    let r = AppRef {
        cell: Arc::new(OnceLock::new()),
        initial: Arc::new(settings.clone()),
    };
    let devices = Arc::new(anotalo_whisper::devices());
    let model = model_manager(&r, notifier.clone(), devices.clone());
    prune_prints(profile);
    let voices = SpeakerModel::new(models_dir(), Arc::new(HttpFetch::default()));
    voices.ensure();
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let events: EventSink = Arc::new(move |e| {
        if tx.send(e).is_ok() {
            window::post(hwnd_raw, WM_APP_EVENT, 0);
        }
    });
    let app = App::new(
        settings.clone(),
        AppDeps {
            detector: detector(r.clone()),
            sessions: sessions(r.clone(), model.clone(), voices, profile),
            notifier: notifier.clone(),
            model: model.clone(),
            copy_text: Box::new(clipboard::set_text),
            events,
            save: Box::new(move |s| {
                if !can_save {
                    return Err("los ajustes no se pudieron leer al arrancar".into());
                }
                save_settings(s, &cfg_path).map_err(|e| e.to_string())
            }),
        },
    );
    let _ = r.cell.set(Arc::downgrade(&app));
    {
        // "Liberar el modelo tras…" se aplica en vivo (para la ventana de ajustes)
        let m = model.clone();
        app.set_after_apply(
            "release_after_min",
            Box::new(move |v| {
                if let Some(min) = v.as_u64() {
                    m.set_release_after(Duration::from_secs(min.saturating_mul(60)));
                }
            }),
        );
    }
    model.prepare();

    let indicator = Arc::new(Indicator::spawn());
    let wiring = Arc::new(IndicatorWiring::new(
        settings.show_indicator,
        indicator.clone() as Arc<dyn Pill>,
    ));
    // lo que la página pide al hilo principal (atajos, diálogos, la ventana)
    let calls: Arc<MainCalls<Shell>> = Arc::new(MainCalls::new(Box::new(move || {
        window::post(hwnd_raw, WM_APP_CALL, 0);
    })));
    let ui = Arc::new(UiApi::new(UiDeps {
        app: app.clone(),
        live_id: {
            let a = app.clone();
            Box::new(move || live_id(&a))
        },
        copy: Box::new(clipboard::set_text),
        save_dialog: {
            let c = calls.clone();
            Box::new(move |name: &str| {
                let name = name.to_string();
                c.call(None, move |sh: &Shell| {
                    sh.owner()
                        .and_then(|o| file_dialogs::save_markdown(o, &name))
                })
                .flatten()
            })
        },
        folder_dialog: {
            let c = calls.clone();
            Box::new(move || {
                c.call(None, |sh: &Shell| {
                    sh.owner().and_then(file_dialogs::pick_folder)
                })
                .flatten()
            })
        },
        today: Box::new(|| chrono::Local::now().date_naive()),
        device_info: {
            let (m, d, r) = (model.clone(), devices.clone(), r.clone());
            Box::new(move || device_text(&m, &d, &r.settings()))
        },
        hotkey_capture: {
            let c = calls.clone();
            Box::new(move |on| {
                if c.call(Some(MAIN_WAIT), move |sh: &Shell| sh.pause_hotkeys(on))
                    .is_none()
                {
                    let what = if on { "soltar" } else { "devolver" };
                    log::warn!("No se pudieron {what} los atajos a tiempo");
                }
            })
        },
        window: Arc::new(WindowCalls(calls.clone())),
        voices_path: voices_path(profile),
        prints_dir: prints_dir(profile),
        diagnostics: {
            let log = data.join("app.log");
            Box::new(move || diagnostics::collect(&log, live_audio))
        },
        version: env!("CARGO_PKG_VERSION").into(),
    }));
    let (reply_tx, replies) = mpsc::channel::<Reply>();
    let bridge = Rc::new(Bridge::new(
        ui.clone(),
        Box::new(move |r| {
            if reply_tx.send(r).is_ok() {
                window::post(hwnd_raw, WM_APP_UI, 0);
            }
        }),
    ));
    // los ajustes que la ventana cambia en vivo
    app.set_applier(
        "hotkey",
        hotkey_applier(calls.clone(), r.clone(), HotkeyId::Record),
    );
    app.set_applier(
        "hotkey_mark",
        hotkey_applier(calls.clone(), r.clone(), HotkeyId::Mark),
    );
    app.set_applier(
        "autostart",
        Box::new(|_| Err("Solo en la versión instalada".into())),
    );
    app.set_applier(
        "output_dir",
        Box::new(|v| {
            let dir = v.as_str().unwrap_or_default();
            if dir.trim().is_empty() {
                return Err("Elige una carpeta".into());
            }
            std::fs::create_dir_all(dir).map_err(|e| format!("No se pudo crear la carpeta: {e}"))
        }),
    );
    {
        let c = calls.clone();
        app.set_after_apply(
            "output_dir",
            Box::new(move |_| c.post(|sh: &Shell| sh.page(&json!({"type": "recordings"})))),
        );
        let w = wiring.clone();
        app.set_after_apply(
            "show_indicator",
            Box::new(move |v| w.set_enabled(v.as_bool().unwrap_or(true))),
        );
        for key in ["model", "language"] {
            let m = model.clone();
            app.set_after_apply(key, Box::new(move |_| m.reload()));
        }
    }

    let mut hotkeys = Hotkeys::new(hwnd);
    for (id, spec) in [
        (HotkeyId::Record, &settings.hotkey),
        (HotkeyId::Mark, &settings.hotkey_mark),
    ] {
        // `register` ya lo anota en app.log; su mensaje nombra el atajo
        if let Err(e) = hotkeys.register(id, spec) {
            notifier.notify("Atajo no disponible", &e, None);
        }
    }
    let shell = Rc::new(Shell {
        hwnd: hwnd_raw,
        app: app.clone(),
        model: model.clone(),
        rx,
        tray: RefCell::new(None),
        hotkeys: RefCell::new(Some(hotkeys)),
        indicator: RefCell::new(Some(indicator)),
        wiring,
        window: RefCell::new(None),
        ui,
        replies,
        calls,
        devices,
        shown: Cell::new(None),
        tray_failures: Cell::new(0),
        // SAFETY: sin argumentos.
        clicks: Debounce::new(Duration::from_millis(u64::from(unsafe {
            GetDoubleClickTime()
        }))),
        toasts,
        quitting: Cell::new(false),
        taskbar_created: window::taskbar_created(),
        profile,
    });
    window::set_handler(Some(shell.clone()));
    shell.add_tray();
    shell.open_window(start_hidden, bridge, settings.window_rect);
    // SAFETY: temporizador de la ventana propia.
    unsafe { SetTimer(Some(hwnd), MODEL_TIMER, MODEL_TICK_MS, None) };
    // otra Anotalo: se muestra la ventana (o, sin ella, se avisa)
    instance::on_show(
        show_event,
        Box::new(move || window::post(hwnd_raw, WM_APP_SHOW, 0)),
    );

    app.start();
    log::info!(
        "Anotalo iniciado (atajo {}, marcas {}, modo automático {})",
        settings.hotkey,
        settings.hotkey_mark,
        if settings.auto_mode { "sí" } else { "no" }
    );
    window::run();
    window::set_handler(None);
    drop(shell);
    log::info!("Anotalo cerrado");
    log::logger().flush();
    // sin esperar a hilos que ya no hacen falta (el modelo a medio cargar, el detector)
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anotalo_core::routing::PLATFORMS;
    use std::time::Duration;

    fn platform(key: &str) -> &'static Platform {
        PLATFORMS.iter().find(|p| p.key == key).unwrap()
    }

    fn titles(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    /// Al detectar una llamada en el navegador, a app.log solo van los títulos que dicen algo (el
    /// sitio o la reunión): no las pestañas de las demás ventanas (asuntos de correos…).
    #[test]
    fn test_only_matching_titles_are_logged() {
        let t = titles(&[
            "Meet - Revisión semanal - Google Chrome",
            "Re: Factura de octubre - Correo - Google Chrome",
            "Banco - Movimientos - Google Chrome",
        ]);
        let (shown, hidden) = split_titles(platform("chrome"), &t);
        assert_eq!(shown, vec!["Meet - Revisión semanal - Google Chrome"]);
        assert_eq!(hidden.len(), 2);
        let teams = titles(&["Reunión con Acme | Microsoft Teams", "Chat | Contoso"]);
        let (shown, hidden) = split_titles(platform("teams"), &teams);
        assert_eq!(shown, vec!["Reunión con Acme | Microsoft Teams"]);
        assert_eq!(hidden, vec!["Chat | Contoso"]);
    }

    fn slow_toast(d: Duration) -> Toasts {
        Toasts::with(Arc::new(move |_, _, _| {
            std::thread::sleep(d);
            Ok(())
        }))
    }

    /// "Salir" durante una llamada: el aviso "Transcripción guardada" llega a verse.
    #[test]
    fn test_quit_waits_for_toasts_being_shown() {
        let t = slow_toast(Duration::from_millis(300));
        t.notify("Transcripción guardada", "x.md", None);
        let t0 = Instant::now();
        assert!(t.wait(Duration::from_secs(5)));
        assert!(
            t0.elapsed() >= Duration::from_millis(250),
            "{:?}",
            t0.elapsed()
        );
        assert!(t.wait(Duration::ZERO), "ya no queda ninguno");
    }

    #[test]
    fn test_toast_wait_is_bounded() {
        let t = slow_toast(Duration::from_secs(5));
        t.notify("Transcripción guardada", "x.md", None);
        let t0 = Instant::now();
        assert!(!t.wait(Duration::from_millis(200)));
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    }

    /// Un doble clic en el icono son dos clics: la carpeta se abre una vez.
    #[test]
    fn test_double_click_opens_once() {
        let d = Debounce::new(Duration::from_millis(500));
        let t0 = Instant::now();
        assert!(d.ready(t0));
        assert!(!d.ready(t0 + Duration::from_millis(300)));
        assert!(d.ready(t0 + Duration::from_millis(1300)));
    }
}

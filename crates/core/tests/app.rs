//! Port de tests/test_app.py (37 casos) más marcas, títulos y las paradas concurrentes.
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::time::{Duration, Instant};

use anotalo_core::app::{
    App, AppDeps, AppEvent, CALL_LABEL, Recording, SessionKind, SessionRequest, State,
};
use anotalo_core::config::Settings;
use anotalo_core::detector::{Detected, DetectorControl, OnDetected};
use anotalo_core::model::ModelGate;
use anotalo_core::notify::Notifier;
use anotalo_core::paths::Profile;
use anotalo_core::routing::{Audio, PLATFORMS};
use anotalo_core::source::OthersSpec;

fn det(key: &str, label: Option<&str>) -> Detected {
    let p = PLATFORMS.iter().find(|p| p.key == key).unwrap();
    Detected {
        platform: p,
        label: label.unwrap_or(p.name).to_string(),
        exe: p.exes.first().map(|e| e.to_string()),
        title: None,
    }
}

fn teams() -> Detected {
    det("teams", None)
}

fn spec(kind: Audio, exe: Option<&str>, platform: Option<&str>) -> OthersSpec {
    OthersSpec {
        kind,
        exe: exe.map(Into::into),
        platform: platform.map(Into::into),
    }
}

#[derive(Default)]
struct FakeDetector {
    on_start: Mutex<Option<OnDetected>>,
    on_stop: Mutex<Option<OnDetected>>,
    suspended: AtomicBool,
    running: AtomicBool,
    dismissed: AtomicUsize,
    dismissed_in_use: AtomicUsize,
}

impl DetectorControl for FakeDetector {
    fn start(&self) {
        self.running.store(true, Ordering::SeqCst);
    }
    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
    fn suspend(&self) {
        self.suspended.store(true, Ordering::SeqCst);
    }
    fn resume(&self) {
        self.suspended.store(false, Ordering::SeqCst);
    }
    fn dismiss(&self) {
        self.dismissed.fetch_add(1, Ordering::SeqCst);
    }
    fn dismiss_in_use(&self) {
        self.dismissed_in_use.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct Started {
    label: String,
    others: Option<OthersSpec>,
    title: Option<String>,
    stopped: Arc<AtomicBool>,
    at: Instant,
}

struct FakeSession {
    label: String,
    text: String,
    stopped: Arc<AtomicBool>,
    stop_delay: Duration,
    stop_ends: Arc<Mutex<Vec<Instant>>>,
    marks: Arc<AtomicUsize>,
}

impl Recording for FakeSession {
    fn stop(self: Box<Self>) -> io::Result<(PathBuf, String)> {
        std::thread::sleep(self.stop_delay);
        self.stopped.store(true, Ordering::SeqCst);
        self.stop_ends.lock().unwrap().push(Instant::now());
        Ok((PathBuf::from(format!("{}.md", self.label)), self.text))
    }
    fn mark(&self) -> Option<f64> {
        self.marks.fetch_add(1, Ordering::SeqCst);
        Some(75.0)
    }
}

#[derive(Default)]
struct FakeModel {
    not_ready: AtomicBool,
    preparing: Mutex<Option<(f32, String)>>,
    retries: AtomicUsize,
    on_ready: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl ModelGate for FakeModel {
    fn ready(&self) -> bool {
        if let Some(f) = self.on_ready.lock().unwrap().as_ref() {
            f();
        }
        !self.not_ready.load(Ordering::SeqCst)
    }
    fn preparing(&self) -> Option<(f32, String)> {
        self.preparing.lock().unwrap().clone()
    }
    fn request_reload(&self) {
        self.retries.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Notes(Mutex<Vec<(String, String)>>);

impl Notifier for Notes {
    fn notify(&self, title: &str, body: &str, _open: Option<&Path>) {
        self.0.lock().unwrap().push((title.into(), body.into()));
    }
}

type Hook = Box<dyn Fn(&App, &AppEvent) + Send + Sync>;

struct Harness {
    app: Arc<App>,
    det: Arc<FakeDetector>,
    model: Arc<FakeModel>,
    notes: Arc<Notes>,
    sessions: Arc<Mutex<Vec<Started>>>,
    copied: Arc<Mutex<Vec<String>>>,
    events: Arc<Mutex<Vec<AppEvent>>>,
    saved: Arc<AtomicUsize>,
    hook: Arc<Mutex<Option<Hook>>>,
    stop_delay: Arc<Mutex<Duration>>,
    stop_ends: Arc<Mutex<Vec<Instant>>>,
    marks: Arc<AtomicUsize>,
}

fn harness(auto: bool, text: &str, copy_on_stop: bool) -> Harness {
    let det = Arc::new(FakeDetector::default());
    let model = Arc::new(FakeModel::default());
    let notes = Arc::new(Notes::default());
    let sessions = Arc::new(Mutex::new(Vec::<Started>::new()));
    let copied = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let saved = Arc::new(AtomicUsize::new(0));
    let hook: Arc<Mutex<Option<Hook>>> = Arc::new(Mutex::new(None));
    let stop_delay = Arc::new(Mutex::new(Duration::ZERO));
    let stop_ends = Arc::new(Mutex::new(Vec::new()));
    let marks = Arc::new(AtomicUsize::new(0));
    let weak: Arc<OnceLock<Weak<App>>> = Arc::new(OnceLock::new());

    let d = det.clone();
    let (ss, txt, sd, se, mk) = (
        sessions.clone(),
        text.to_string(),
        stop_delay.clone(),
        stop_ends.clone(),
        marks.clone(),
    );
    let (cp, ev, sv, hk, wk) = (
        copied.clone(),
        events.clone(),
        saved.clone(),
        hook.clone(),
        weak.clone(),
    );
    let deps = AppDeps {
        detector: Box::new(move |on_start, on_stop| {
            *d.on_start.lock().unwrap() = Some(on_start);
            *d.on_stop.lock().unwrap() = Some(on_stop);
            d as Arc<dyn DetectorControl>
        }),
        sessions: Box::new(move |req: SessionRequest, _events| {
            let stopped = Arc::new(AtomicBool::new(false));
            ss.lock().unwrap().push(Started {
                label: req.label.clone(),
                others: req.others.clone(),
                title: req.title.clone(),
                stopped: stopped.clone(),
                at: Instant::now(),
            });
            Ok(Box::new(FakeSession {
                label: req.label,
                text: txt.clone(),
                stopped,
                stop_delay: *sd.lock().unwrap(),
                stop_ends: se.clone(),
                marks: mk.clone(),
            }) as Box<dyn Recording>)
        }),
        notifier: notes.clone(),
        model: model.clone(),
        copy_text: Box::new(move |t| {
            cp.lock().unwrap().push(t.to_string());
            Ok(())
        }),
        events: Arc::new(move |e: AppEvent| {
            if let (Some(f), Some(app)) = (
                hk.lock().unwrap().as_ref(),
                wk.get().and_then(Weak::upgrade),
            ) {
                f(&app, &e);
            }
            ev.lock().unwrap().push(e);
        }),
        save: Box::new(move |_s| {
            sv.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    };
    let settings = Settings {
        auto_mode: auto,
        copy_on_stop,
        ..Settings::defaults(Profile::Dev)
    };
    let app = App::new(settings, deps);
    weak.set(Arc::downgrade(&app)).ok();
    app.start();
    Harness {
        app,
        det,
        model,
        notes,
        sessions,
        copied,
        events,
        saved,
        hook,
        stop_delay,
        stop_ends,
        marks,
    }
}

fn h() -> Harness {
    harness(true, "texto de Yo", true)
}

impl Harness {
    fn on_start(&self, d: Detected) {
        let cb = self.det.on_start.lock().unwrap();
        cb.as_ref().unwrap()(d);
    }
    fn on_stop(&self, d: Detected) {
        let cb = self.det.on_stop.lock().unwrap();
        cb.as_ref().unwrap()(d);
    }
    fn session(&self, i: usize) -> Started {
        self.sessions.lock().unwrap()[i].clone()
    }
    fn count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }
    fn stopped(&self, i: usize) -> bool {
        self.session(i).stopped.load(Ordering::SeqCst)
    }
    fn suspended(&self) -> bool {
        self.det.suspended.load(Ordering::SeqCst)
    }
    fn notes(&self) -> Vec<(String, String)> {
        self.notes.0.lock().unwrap().clone()
    }
    fn copied(&self) -> Vec<String> {
        self.copied.lock().unwrap().clone()
    }
    fn set_hook(&self, f: Hook) {
        *self.hook.lock().unwrap() = Some(f);
    }
    fn ended(&self) -> Vec<(Option<PathBuf>, String, bool)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AppEvent::SessionEnded {
                    path,
                    label,
                    copied,
                } => Some((path.clone(), label.clone(), *copied)),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn test_toggle_starts_and_stops() {
    let h = h();
    h.app.toggle();
    assert_eq!(h.app.state(), State::Recording);
    assert_eq!(h.session(0).label, "Nota");
    h.app.toggle();
    assert!(h.stopped(0));
    assert_eq!(h.app.state(), State::Armed);
}

#[test]
fn test_manual_suspends_detector() {
    let h = h();
    h.app.toggle();
    assert!(h.suspended());
    h.app.toggle();
    assert!(!h.suspended());
}

#[test]
fn test_auto_stop_ignores_manual_session() {
    let h = h();
    h.app.toggle();
    h.on_stop(teams());
    assert!(!h.stopped(0));
}

#[test]
fn test_auto_start_teams_records_its_output_device() {
    // Capturar solo el proceso de Teams da silencio en algunos PCs: se graba su dispositivo de salida
    let h = h();
    h.on_start(teams());
    let s = h.session(0);
    assert_eq!(
        (s.label.as_str(), s.others),
        ("Teams", Some(spec(Audio::Device, None, Some("teams"))))
    );
    h.on_stop(teams());
    assert!(h.stopped(0));
}

#[test]
fn test_meet_uses_firefox() {
    let h = h();
    h.on_start(det("firefox", Some("Meet")));
    assert_eq!(
        h.session(0).others,
        Some(spec(Audio::Process, Some("firefox.exe"), Some("firefox")))
    );
}

#[test]
fn test_hotkey_stop_of_auto_session_dismisses() {
    let h = h();
    h.on_start(teams());
    h.app.toggle();
    assert!(h.stopped(0));
    assert_eq!(h.det.dismissed.load(Ordering::SeqCst), 1);
}

#[test]
fn test_auto_disabled_suspends_detector() {
    let h = harness(false, "texto de Yo", true);
    assert!(h.suspended());
    assert_eq!(h.app.state(), State::Idle);
    h.app.set_auto(true);
    assert!(!h.suspended());
    assert_eq!(h.app.state(), State::Armed);
}

#[test]
fn test_quit_stops_session_and_saves() {
    let h = h();
    h.app.toggle();
    h.app.quit();
    assert!(h.stopped(0));
    assert!(!h.det.running.load(Ordering::SeqCst));
}

#[test]
fn test_model_wait_does_not_hold_lock() {
    // Mientras se pregunta si el modelo está listo, otro hilo puede leer el estado de la app.
    let h = h();
    let free = Arc::new(Mutex::new(Vec::new()));
    let (app, f) = (Arc::downgrade(&h.app), free.clone());
    *h.model.on_ready.lock().unwrap() = Some(Box::new(move || {
        let app = app.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Some(a) = app.upgrade() {
                let _ = a.state();
                let _ = a.recording_active();
            }
            let _ = tx.send(());
        });
        f.lock()
            .unwrap()
            .push(rx.recv_timeout(Duration::from_millis(500)).is_ok());
    }));
    h.on_start(teams());
    assert_eq!(*free.lock().unwrap(), vec![true]);
    assert_eq!(h.count(), 1);
}

#[test]
fn test_no_session_after_quit() {
    let h = h();
    h.app.quit();
    h.on_start(teams());
    h.app.toggle();
    assert_eq!(h.count(), 0);
}

#[test]
fn test_note_copied_on_stop() {
    let h = h();
    h.app.toggle();
    h.app.toggle();
    assert_eq!(h.copied(), vec!["texto de Yo"]);
}

#[test]
fn test_call_not_copied() {
    let h = h();
    h.on_start(teams());
    h.app.toggle();
    assert!(h.copied().is_empty());
}

#[test]
fn test_empty_note_not_copied() {
    let h = harness(true, "", true);
    h.app.toggle();
    h.app.toggle();
    assert!(h.copied().is_empty());
    assert!(h.stopped(0));
}

#[test]
fn test_copy_disabled_setting() {
    let h = harness(true, "texto de Yo", false);
    h.app.toggle();
    h.app.toggle();
    assert!(h.copied().is_empty());
}

#[test]
fn test_session_end_event() {
    let h = h();
    h.app.toggle();
    h.app.toggle();
    assert_eq!(
        h.ended(),
        vec![(Some(PathBuf::from("Nota.md")), "Nota".to_string(), true)]
    );
}

#[test]
fn test_manual_label_is_nota() {
    let h = h();
    h.app.toggle();
    let s = h.session(0);
    assert_eq!((s.label.as_str(), s.others), ("Nota", None));
}

#[test]
fn test_apply_setting_rejected_while_recording() {
    let h = h();
    h.app.toggle();
    for (key, val) in [
        ("model", "small"),
        ("output_dir", "C:/x"),
        ("language", "en"),
    ] {
        assert_eq!(
            h.app.apply_setting(key, serde_json::json!(val)),
            Err("No se puede cambiar mientras se graba".to_string())
        );
    }
    assert_eq!(h.app.settings().model, "auto");
}

#[test]
fn test_apply_hotkey_calls_rebind() {
    let h = h();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let c = calls.clone();
    h.app.set_applier(
        "hotkey",
        Box::new(move |v| {
            c.lock().unwrap().push(v.clone());
            Ok(())
        }),
    );
    assert_eq!(
        h.app
            .apply_setting("hotkey", serde_json::json!("<ctrl>+<alt>+n")),
        Ok(())
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![serde_json::json!("<ctrl>+<alt>+n")]
    );
    assert_eq!(h.app.settings().hotkey, "<ctrl>+<alt>+n");
}

#[test]
fn test_apply_hotkey_error_keeps_old() {
    let h = h();
    h.app.set_applier(
        "hotkey",
        Box::new(|_| Err("Esa combinación ya la usa otra aplicación".into())),
    );
    assert_eq!(
        h.app
            .apply_setting("hotkey", serde_json::json!("<ctrl>+<alt>+n")),
        Err("Esa combinación ya la usa otra aplicación".to_string())
    );
    assert_eq!(h.app.settings().hotkey, "<ctrl>+<alt>+r");
}

#[test]
fn test_apply_unknown_key() {
    let h = h();
    let err = h
        .app
        .apply_setting("nope", serde_json::json!(1))
        .unwrap_err();
    assert!(err.contains("desconocido"), "{err}");
}

#[test]
fn test_apply_auto_mode_and_saves() {
    let h = h();
    assert_eq!(
        h.app.apply_setting("auto_mode", serde_json::json!(false)),
        Ok(())
    );
    assert!(h.suspended());
    assert_eq!(h.app.state(), State::Idle);
    assert!(h.saved.load(Ordering::SeqCst) > 0);
}

#[test]
fn test_stopping_event_before_stop() {
    let h = h();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s, sessions) = (seen.clone(), h.sessions.clone());
    h.set_hook(Box::new(move |_app, e| {
        if let AppEvent::SessionStopping { label } = e {
            let stopped = sessions.lock().unwrap()[0].stopped.load(Ordering::SeqCst);
            s.lock().unwrap().push((label.clone(), stopped));
        }
    }));
    h.app.toggle();
    h.app.toggle();
    assert_eq!(*seen.lock().unwrap(), vec![("Nota".to_string(), false)]);
}

#[test]
fn test_after_apply_runs_with_new_value() {
    let h = h();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s, app) = (seen.clone(), Arc::downgrade(&h.app));
    h.app.set_after_apply(
        "model",
        Box::new(move |_v| {
            s.lock()
                .unwrap()
                .push(app.upgrade().unwrap().settings().model);
        }),
    );
    assert_eq!(
        h.app.apply_setting("model", serde_json::json!("small")),
        Ok(())
    );
    assert_eq!(*seen.lock().unwrap(), vec!["small"]);
}

#[test]
fn test_setting_blocked_while_stopping() {
    let h = h();
    h.app.toggle();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    h.set_hook(Box::new(move |app, e| {
        if matches!(e, AppEvent::SessionStopping { .. }) {
            s.lock()
                .unwrap()
                .push(app.apply_setting("model", serde_json::json!("small")));
        }
    }));
    h.app.toggle();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![Err("No se puede cambiar mientras se graba".to_string())]
    );
    assert!(!h.app.recording_active());
}

#[test]
fn test_toggle_while_preparing_notifies_no_session() {
    let h = h();
    *h.model.preparing.lock().unwrap() = Some((0.4, "Descargando".into()));
    h.model.not_ready.store(true, Ordering::SeqCst);
    h.app.toggle();
    assert_eq!(h.count(), 0);
    assert!(
        h.notes()
            .iter()
            .any(|n| n.0.to_lowercase().contains("preparando"))
    );
}

#[test]
fn test_call_start_while_preparing_ignored() {
    let h = h();
    *h.model.preparing.lock().unwrap() = Some((0.1, "Descargando".into()));
    h.model.not_ready.store(true, Ordering::SeqCst);
    h.on_start(teams());
    assert_eq!(h.count(), 0);
}

#[test]
fn test_state_preparing() {
    let h = h();
    *h.model.preparing.lock().unwrap() = Some((0.5, "x".into()));
    h.app.model_changed();
    assert_eq!(h.app.state(), State::Preparing);
    assert_eq!(
        h.events.lock().unwrap().last(),
        Some(&AppEvent::State(State::Preparing))
    );
    *h.model.preparing.lock().unwrap() = None;
    h.app.model_changed();
    assert_eq!(h.app.state(), State::Armed);
}

#[test]
fn test_non_blocking_progress_keeps_recording_available() {
    // El modelo se liberó y se recarga al grabar: no hay preparación que bloquee.
    let h = h();
    assert!(h.model.preparing().is_none());
    assert_eq!(h.app.state(), State::Armed);
    h.app.toggle();
    assert_eq!(h.count(), 1);
}

#[test]
fn test_failed_model_retries_on_hotkey() {
    let h = h();
    h.model.not_ready.store(true, Ordering::SeqCst); // no hay modelo y no se está preparando
    h.app.toggle();
    assert_eq!(h.count(), 0);
    assert_eq!(h.model.retries.load(Ordering::SeqCst), 1);
    assert!(
        h.notes()
            .iter()
            .any(|n| (n.0.clone() + &n.1).to_lowercase().contains("reintent"))
    );
}

#[test]
fn test_manual_call_uses_device_audio_and_call_label() {
    let h = h();
    h.app.start_call_manual();
    let s = h.session(0);
    assert_eq!(s.label, CALL_LABEL);
    assert_eq!(CALL_LABEL, "Llamada");
    assert_eq!(s.others, Some(spec(Audio::Device, None, None)));
    assert_eq!(h.app.session_kind(), Some(SessionKind::Manual));
}

#[test]
fn test_manual_call_suspends_detector_and_hotkey_stops_it() {
    let h = harness(true, "hola", true);
    h.app.start_call_manual();
    assert!(h.suspended());
    h.app.toggle(); // el atajo de notas para la llamada
    assert!(h.stopped(0));
    assert!(h.copied().is_empty()); // las llamadas nunca se copian
    assert_eq!(h.app.session_kind(), None);
    assert!(!h.suspended());
}

#[test]
fn test_manual_call_not_started_while_recording() {
    let h = h();
    h.app.toggle(); // nota rápida en curso
    h.app.start_call_manual();
    assert_eq!(h.count(), 1);
    assert_eq!(h.app.session_kind(), Some(SessionKind::Note));
}

#[test]
fn test_toggle_call_starts_and_stops() {
    let h = h();
    h.app.toggle_call();
    assert_eq!(h.app.session_kind(), Some(SessionKind::Manual));
    h.app.toggle_call();
    assert!(h.stopped(0));
    assert_eq!(h.app.session_kind(), None);
}

#[test]
fn test_detected_platform_label_and_audio() {
    let h = h();
    h.on_start(det("whatsapp", None));
    let s = h.session(0);
    assert_eq!(
        (s.label.as_str(), s.others),
        (
            "WhatsApp",
            Some(spec(Audio::Device, None, Some("whatsapp")))
        )
    );
    assert_eq!(h.app.session_kind(), Some(SessionKind::Call));
    h.on_stop(det("whatsapp", None));
    h.on_start(det("zoom", None));
    assert_eq!(
        h.session(1).others,
        Some(spec(Audio::Process, Some("zoom.exe"), Some("zoom")))
    );
}

#[test]
fn test_recording_active_includes_saving() {
    let h = h();
    assert!(!h.app.recording_active());
    h.app.toggle();
    assert!(h.app.recording_active());
    *h.stop_delay.lock().unwrap() = Duration::from_millis(300);
    // la sesión ya estaba creada con retraso 0: se crea otra que tarde en cerrar
    h.app.toggle();
    h.app.toggle();
    let app = h.app.clone();
    let stopper = std::thread::spawn(move || app.toggle());
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        h.app.recording_active(),
        "cerrando la grabación (últimas frases)"
    );
    stopper.join().unwrap();
    assert!(!h.app.recording_active());
}

#[test]
fn test_stopping_manual_call_dismisses_platforms_in_use() {
    let h = h();
    h.app.start_call_manual();
    h.app.toggle_call();
    assert_eq!(h.det.dismissed_in_use.load(Ordering::SeqCst), 1);
    h.app.toggle(); // una nota rápida no descarta nada
    h.app.toggle();
    assert_eq!(h.det.dismissed_in_use.load(Ordering::SeqCst), 1);
}

#[test]
fn test_no_new_recording_while_updating() {
    // Tras lanzar el instalador, la app se cerrará en segundos: no empezar grabaciones que se perderían.
    let h = h();
    h.app.set_updating(true);
    h.app.toggle();
    h.app.start_call_manual();
    h.on_start(teams());
    assert_eq!(h.count(), 0);
    assert!(h.notes().iter().any(|n| n.0.contains("Actualizando")));
}

#[test]
fn test_mark_during_call_writes_and_emits() {
    let h = h();
    h.on_start(teams());
    assert!(h.app.mark());
    assert_eq!(h.marks.load(Ordering::SeqCst), 1);
    assert!(h.events.lock().unwrap().contains(&AppEvent::Marked {
        hms: "00:01:15".into()
    }));
    h.app.toggle();
    h.app.start_call_manual();
    assert!(h.app.mark(), "también en una llamada grabada a mano");
}

#[test]
fn test_mark_during_note_does_nothing() {
    let h = h();
    assert!(!h.app.mark(), "sin grabación");
    h.app.toggle();
    assert!(!h.app.mark(), "en una nota rápida");
    assert_eq!(h.marks.load(Ordering::SeqCst), 0);
}

#[test]
fn test_hotkey_while_saving_does_not_start_a_second_session() {
    let h = h();
    *h.stop_delay.lock().unwrap() = Duration::from_millis(300);
    h.app.toggle(); // nota que tarda 300 ms en cerrarse
    let app = h.app.clone();
    let first = std::thread::spawn(move || app.toggle()); // la para
    std::thread::sleep(Duration::from_millis(50));
    let app = h.app.clone();
    let second = std::thread::spawn(move || app.toggle()); // se pulsa otra vez mientras se guarda
    first.join().unwrap();
    second.join().unwrap();
    let ends = h.stop_ends.lock().unwrap().clone();
    let sessions = h.sessions.lock().unwrap().clone();
    assert!(sessions.len() <= 2);
    if sessions.len() == 2 {
        assert!(
            sessions[1].at >= ends[0],
            "la segunda empezó antes de cerrarse la primera"
        );
    }
}

#[test]
fn test_quit_during_saving_waits_for_the_file() {
    let h = h();
    *h.stop_delay.lock().unwrap() = Duration::from_millis(300);
    h.app.toggle();
    let app = h.app.clone();
    let stopper = std::thread::spawn(move || app.toggle());
    std::thread::sleep(Duration::from_millis(50));
    h.app.quit();
    assert!(
        h.stopped(0),
        "quit vuelve cuando el archivo ya está cerrado"
    );
    stopper.join().unwrap();
}

#[test]
fn test_detected_title_reaches_the_session() {
    let h = h();
    h.on_start(Detected {
        title: Some("Reunión con Acme".into()),
        ..teams()
    });
    assert_eq!(h.session(0).title.as_deref(), Some("Reunión con Acme"));
}

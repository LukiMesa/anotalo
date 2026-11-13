//! Una `App` de prueba (detector, grabaciones, avisos y modelo de mentira), compartida por los
//! tests de la app y de las órdenes de la ventana.
#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use anotalo_core::app::{App, AppDeps, AppEvent, Recording, SessionRequest};
use anotalo_core::config::Settings;
use anotalo_core::detector::{Detected, DetectorControl, OnDetected};
use anotalo_core::model::ModelGate;
use anotalo_core::notify::Notifier;
use anotalo_core::paths::Profile;
use anotalo_core::session::Hurry;
use anotalo_core::source::OthersSpec;

#[derive(Default)]
pub struct FakeDetector {
    pub on_start: Mutex<Option<OnDetected>>,
    pub on_stop: Mutex<Option<OnDetected>>,
    pub suspended: AtomicBool,
    pub running: AtomicBool,
    pub dismissed: AtomicUsize,
    pub dismissed_in_use: AtomicUsize,
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
pub struct Started {
    pub label: String,
    pub others: Option<OthersSpec>,
    pub title: Option<String>,
    pub stopped: Arc<AtomicBool>,
    pub at: Instant,
}

pub struct FakeSession {
    label: String,
    text: String,
    stopped: Arc<AtomicBool>,
    stop_delay: Duration,
    stop_ends: Arc<Mutex<Vec<Instant>>>,
    marks: Arc<AtomicUsize>,
}

impl Recording for FakeSession {
    fn stop(self: Box<Self>, hurry: &Hurry) -> io::Result<(PathBuf, String)> {
        // como la de verdad: tarda lo suyo, salvo que haya prisa y venza el plazo
        let end = Instant::now() + self.stop_delay;
        while Instant::now() < end && hurry.deadline().is_none_or(|d| Instant::now() < d) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.stopped.store(true, Ordering::SeqCst);
        self.stop_ends.lock().unwrap().push(Instant::now());
        Ok((PathBuf::from(format!("{}.md", self.label)), self.text))
    }
    fn mark(&self) -> Option<f64> {
        self.marks.fetch_add(1, Ordering::SeqCst);
        Some(75.0)
    }
    fn path(&self) -> Option<PathBuf> {
        Some(PathBuf::from(format!("{}.md", self.label)))
    }
}

#[derive(Default)]
pub struct FakeModel {
    pub not_ready: AtomicBool,
    pub preparing: Mutex<Option<(f32, String)>>,
    pub retries: AtomicUsize,
    pub on_ready: Mutex<Option<Box<dyn Fn() + Send>>>,
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
pub struct Notes(pub Mutex<Vec<(String, String)>>);

impl Notifier for Notes {
    fn notify(&self, title: &str, body: &str, _open: Option<&Path>) {
        self.0.lock().unwrap().push((title.into(), body.into()));
    }
}

pub type Hook = Box<dyn Fn(&App, &AppEvent) + Send + Sync>;
pub type SaveHook = Arc<dyn Fn(&Settings) + Send + Sync>;

pub struct Harness {
    pub app: Arc<App>,
    pub det: Arc<FakeDetector>,
    pub model: Arc<FakeModel>,
    pub notes: Arc<Notes>,
    pub sessions: Arc<Mutex<Vec<Started>>>,
    pub copied: Arc<Mutex<Vec<String>>>,
    pub events: Arc<Mutex<Vec<AppEvent>>>,
    pub saved: Arc<AtomicUsize>,
    /// Lo que se guardó, en orden.
    pub saves: Arc<Mutex<Vec<Settings>>>,
    /// Se llama al guardar, antes de anotarlo.
    pub save_hook: Arc<Mutex<Option<SaveHook>>>,
    pub hook: Arc<Mutex<Option<Hook>>>,
    pub stop_delay: Arc<Mutex<Duration>>,
    pub stop_ends: Arc<Mutex<Vec<Instant>>>,
    pub marks: Arc<AtomicUsize>,
}

pub fn harness(auto: bool, text: &str, copy_on_stop: bool) -> Harness {
    let settings = Settings {
        auto_mode: auto,
        copy_on_stop,
        ..Settings::defaults(Profile::Dev)
    };
    harness_with(settings, text)
}

/// Una app ya arrancada con estos ajustes; cada grabación devuelve `text` al pararla.
pub fn harness_with(settings: Settings, text: &str) -> Harness {
    let det = Arc::new(FakeDetector::default());
    let model = Arc::new(FakeModel::default());
    let notes = Arc::new(Notes::default());
    let sessions = Arc::new(Mutex::new(Vec::<Started>::new()));
    let copied = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let saved = Arc::new(AtomicUsize::new(0));
    let saves = Arc::new(Mutex::new(Vec::new()));
    let save_hook: Arc<Mutex<Option<SaveHook>>> = Arc::new(Mutex::new(None));
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
    let (sl, sh) = (saves.clone(), save_hook.clone());
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
        save: Box::new(move |s| {
            sv.fetch_add(1, Ordering::SeqCst);
            // se suelta antes de llamarlo: un guardado a la vez lo decide la app, no el test
            let hook = sh.lock().unwrap().clone();
            if let Some(f) = hook {
                f(s);
            }
            sl.lock().unwrap().push(s.clone());
            Ok(())
        }),
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
        saves,
        save_hook,
        hook,
        stop_delay,
        stop_ends,
        marks,
    }
}

pub fn h() -> Harness {
    harness(true, "texto de Yo", true)
}

impl Harness {
    pub fn on_start(&self, d: Detected) {
        let cb = self.det.on_start.lock().unwrap();
        cb.as_ref().unwrap()(d);
    }
    pub fn on_stop(&self, d: Detected) {
        let cb = self.det.on_stop.lock().unwrap();
        cb.as_ref().unwrap()(d);
    }
    pub fn session(&self, i: usize) -> Started {
        self.sessions.lock().unwrap()[i].clone()
    }
    pub fn count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }
    pub fn stopped(&self, i: usize) -> bool {
        self.session(i).stopped.load(Ordering::SeqCst)
    }
    pub fn suspended(&self) -> bool {
        self.det.suspended.load(Ordering::SeqCst)
    }
    pub fn notes(&self) -> Vec<(String, String)> {
        self.notes.0.lock().unwrap().clone()
    }
    pub fn copied(&self) -> Vec<String> {
        self.copied.lock().unwrap().clone()
    }
    pub fn set_hook(&self, f: Hook) {
        *self.hook.lock().unwrap() = Some(f);
    }
    pub fn ended(&self) -> Vec<(Option<PathBuf>, String, bool)> {
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

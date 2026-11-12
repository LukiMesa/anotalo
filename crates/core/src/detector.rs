//! Detección de llamadas (port de calltx/detector.py).
//!
//! Una llamada empieza cuando una de las apps de `routing::PLATFORMS` usa el micrófono unos
//! segundos seguidos (WhatsApp, más: sus notas de voz también lo usan). Termina SOLO cuando esa app
//! suelta el micro durante el periodo de gracia: cambiar de pestaña o de app no corta la grabación.
//! "Usa el micro" = el registro de privacidad de Windows (ConsentStore) o una sesión de captura de
//! Core Audio de esa app (spec §4.4).

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::routing::{PLATFORMS, Platform, platform_for_key, site_from_title};

pub const START_HOLD_S: f64 = 4.0;
pub const STOP_GRACE_S: f64 = 10.0;
pub const POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub platform: &'static Platform,
    /// "Teams", "Meet", "Chrome"…: va al nombre del archivo.
    pub label: String,
    /// Proceso cuyo audio es "los demás" (si la plataforma usa el audio del proceso).
    pub exe: Option<String>,
    /// Título de la reunión, si su ventana lo dice.
    pub title: Option<String>,
}

pub type OnDetected = Box<dyn Fn(Detected) + Send + Sync>;

pub struct Probes {
    /// Claves de las apps que usan el micro (registro ∪ Core Audio).
    pub mic: Box<dyn Fn() -> HashSet<String> + Send>,
    /// Títulos de las ventanas visibles de esa plataforma.
    pub windows: Box<dyn Fn(&'static Platform) -> Vec<String> + Send>,
}

enum Event {
    Start(Detected),
    Stop(Detected),
}

type Callback = Arc<dyn Fn(Detected) + Send + Sync>;

pub struct CallDetector {
    on_start: Callback,
    on_stop: Callback,
    probes: Probes,
    enabled: Box<dyn Fn(&Platform) -> bool + Send>,
    clock: Box<dyn Fn() -> f64 + Send>,
    suspended: bool,
    candidate: Option<&'static str>,
    candidate_since: f64,
    active: Option<Detected>,
    released_since: Option<f64>,
    /// Plataformas paradas a mano: no cuentan hasta que suelten el micro (clave -> desde cuándo).
    dismissed: HashMap<&'static str, Option<f64>>,
    last_in_use: HashSet<String>,
}

impl CallDetector {
    pub fn new(
        on_start: OnDetected,
        on_stop: OnDetected,
        probes: Probes,
        enabled: Box<dyn Fn(&Platform) -> bool + Send>,
        clock: Box<dyn Fn() -> f64 + Send>,
    ) -> CallDetector {
        CallDetector {
            on_start: Arc::from(on_start),
            on_stop: Arc::from(on_stop),
            probes,
            enabled,
            clock,
            suspended: false,
            candidate: None,
            candidate_since: 0.0,
            active: None,
            released_since: None,
            dismissed: HashMap::new(),
            last_in_use: HashSet::new(),
        }
    }

    pub fn active(&self) -> Option<&Detected> {
        self.active.as_ref()
    }

    pub fn suspend(&mut self) {
        self.suspended = true;
        self.candidate = None;
    }

    pub fn resume(&mut self) {
        self.suspended = false;
    }

    /// Una vuelta del sondeo; avisa de inicios y finales.
    pub fn tick(&mut self) {
        let (start, stop) = (self.on_start.clone(), self.on_stop.clone());
        for e in self.poll() {
            match e {
                Event::Start(d) => start(d),
                Event::Stop(d) => stop(d),
            }
        }
    }

    fn callbacks(&self) -> (Callback, Callback) {
        (self.on_start.clone(), self.on_stop.clone())
    }

    fn poll(&mut self) -> Vec<Event> {
        let now = (self.clock)();
        let keys = (self.probes.mic)();
        if keys != self.last_in_use {
            let mut list: Vec<&String> = keys.iter().collect();
            list.sort();
            log::info!("Micrófono en uso por: {list:?}");
            self.last_in_use = keys.clone();
        }
        let in_use = platforms_in_use(&keys);
        let grace = STOP_GRACE_S;
        self.dismissed.retain(|key, since| {
            if in_use.contains(key) {
                *since = None;
                true
            } else {
                match *since {
                    None => {
                        *since = Some(now);
                        true
                    }
                    Some(s) => now - s < grace, // esa llamada terminó de verdad
                }
            }
        });
        if let Some(active) = &self.active {
            if in_use.contains(active.platform.key) {
                self.released_since = None;
            } else if let Some(since) = self.released_since {
                if now - since >= grace {
                    let det = self.active.take().unwrap();
                    self.released_since = None;
                    log::info!("Fin de llamada {}", det.label);
                    return vec![Event::Stop(det)];
                }
            } else {
                self.released_since = Some(now);
            }
            return Vec::new();
        }
        if self.suspended {
            return Vec::new();
        }
        let cand = PLATFORMS.iter().find(|p| {
            in_use.contains(p.key) && !self.dismissed.contains_key(p.key) && (self.enabled)(p)
        });
        let Some(cand) = cand else {
            self.candidate = None;
            return Vec::new();
        };
        if self.candidate != Some(cand.key) {
            self.candidate = Some(cand.key);
            self.candidate_since = now;
            return Vec::new();
        }
        if now - self.candidate_since < cand.start_hold_s.unwrap_or(START_HOLD_S) {
            return Vec::new();
        }
        self.candidate = None;
        let det = self.describe(cand);
        log::info!(
            "Llamada detectada: {} ({}){}",
            det.label,
            cand.name,
            det.title
                .as_deref()
                .map_or(String::new(), |t| format!(" «{t}»"))
        );
        self.active = Some(det.clone());
        vec![Event::Start(det)]
    }

    fn describe(&self, p: &'static Platform) -> Detected {
        let titles = (self.probes.windows)(p);
        let label = if p.browser {
            titles
                .iter()
                .find_map(|t| site_from_title(t))
                .unwrap_or(p.name)
                .to_string()
        } else {
            p.name.to_string()
        };
        Detected {
            platform: p,
            label,
            exe: p.exes.first().map(|e| e.to_string()),
            title: meeting_title(p, &titles),
        }
    }

    /// La grabación automática se paró a mano: esa app no cuenta (ni avisa del fin) hasta que suelte
    /// el micro. Las demás apps se siguen detectando.
    pub fn dismiss(&mut self) {
        if let Some(det) = self.active.take() {
            self.dismissed.insert(det.platform.key, None);
            self.released_since = None;
            log::info!(
                "Llamada {} descartada hasta que suelte el micrófono",
                det.label
            );
        }
    }

    /// Una llamada grabada a mano se paró: las apps que usan el micro ahora (esa misma llamada) no
    /// empiezan otra grabación hasta que lo suelten.
    pub fn dismiss_in_use(&mut self) {
        for key in platforms_in_use(&(self.probes.mic)()) {
            self.dismissed.insert(key, None);
        }
    }
}

/// Claves de las apps que usan el micro: el registro (`LastUsedTimeStop == 0` tras haberlo usado)
/// más lo que Core Audio dice que está grabando. Si una fuente falla, vale la otra.
pub fn mic_keys(
    registry: Result<HashMap<String, (u64, u64)>, String>,
    capture: Result<HashSet<String>, String>,
) -> HashSet<String> {
    let (reg, audio) = split_sources(registry, capture);
    reg.union(&audio).cloned().collect()
}

fn split_sources(
    registry: Result<HashMap<String, (u64, u64)>, String>,
    capture: Result<HashSet<String>, String>,
) -> (HashSet<String>, HashSet<String>) {
    let reg = match registry {
        Ok(entries) => entries
            .into_iter()
            .filter(|(_, (start, stop))| *start > 0 && *stop == 0)
            .map(|(k, _)| k)
            .collect(),
        Err(e) => {
            log::debug!("No se pudo leer el registro del micrófono: {e}");
            HashSet::new()
        }
    };
    let audio = capture.unwrap_or_else(|e| {
        log::debug!("No se pudo leer el audio de Windows: {e}");
        HashSet::new()
    });
    (reg, audio)
}

/// Como `mic_keys`, y además anota (una vez por cambio) las plataformas que solo Core Audio ve: en
/// un PC el registro no anotaba a Teams aunque grababa.
#[derive(Default)]
pub struct MicProbe {
    audio_only: Mutex<HashSet<&'static str>>,
}

impl MicProbe {
    pub fn read(
        &self,
        registry: Result<HashMap<String, (u64, u64)>, String>,
        capture: Result<HashSet<String>, String>,
    ) -> HashSet<String> {
        let (reg, audio) = split_sources(registry, capture);
        let only: HashSet<&'static str> = platforms_in_use(&audio)
            .difference(&platforms_in_use(&reg))
            .copied()
            .collect();
        let mut last = self.audio_only.lock().unwrap_or_else(|e| e.into_inner());
        if *last != only {
            if !only.is_empty() {
                let mut list: Vec<&str> = only.iter().copied().collect();
                list.sort();
                log::info!(
                    "Micrófono en uso por {} según el audio de Windows (el registro no lo anota)",
                    list.join(", ")
                );
            }
            *last = only;
        }
        reg.union(&audio).cloned().collect()
    }
}

pub fn platforms_in_use(keys: &HashSet<String>) -> HashSet<&'static str> {
    keys.iter()
        .filter_map(|k| platform_for_key(k))
        .map(|p| p.key)
        .collect()
}

const BROWSERS: &[&str] = &[
    "Google Chrome",
    "Mozilla Firefox",
    "Brave",
    "Microsoft Edge",
    "Edge",
    "Opera",
    "Vivaldi",
];

/// Secciones de Teams que no son una reunión.
const TEAMS_SECTIONS: &[&str] = &[
    "chat",
    "actividad",
    "activity",
    "calendario",
    "calendar",
    "llamadas",
    "calls",
    "archivos",
    "files",
    "teams",
    "equipos",
    "aplicaciones",
    "apps",
    "reunirse",
    "meet",
    "copilot",
    "personas",
    "people",
];

/// Título de la reunión según las ventanas de la plataforma (spec §2.4.2), o None.
pub fn meeting_title(platform: &Platform, window_titles: &[String]) -> Option<String> {
    window_titles.iter().find_map(|t| title_of(platform, t))
}

fn title_of(p: &Platform, raw: &str) -> Option<String> {
    let mut t = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if p.browser {
        t = strip_browser_suffix(&t);
    }
    if t.ends_with("Microsoft Teams") && (p.key == "teams" || p.browser) {
        let parts: Vec<&str> = t.split(" | ").map(str::trim).collect();
        if parts.len() < 2 || parts.last() != Some(&"Microsoft Teams") {
            return None;
        }
        let first = parts[0];
        if first.is_empty() || TEAMS_SECTIONS.contains(&first.to_lowercase().as_str()) {
            return None;
        }
        return Some(first.to_string());
    }
    if p.browser {
        let rest = ["Meet - ", "Meet – ", "Meet — ", "Meet: "]
            .iter()
            .find_map(|pre| t.strip_prefix(pre))?
            .trim();
        let code = regex::Regex::new(r"^[a-z]{3}-[a-z]{4}-[a-z]{3}$").expect("regex válida");
        if rest.is_empty() || code.is_match(rest) {
            return None;
        }
        return Some(rest.to_string());
    }
    None
}

/// Quita " - Google Chrome", " — Mozilla Firefox"… del final del título de una pestaña.
fn strip_browser_suffix(t: &str) -> String {
    let cut = [" - ", " — ", " – "]
        .iter()
        .filter_map(|sep| t.rfind(sep).map(|i| (i, sep.len())))
        .max_by_key(|(i, _)| *i);
    match cut {
        Some((i, len)) if BROWSERS.iter().any(|b| t[i + len..].contains(b)) => {
            t[..i].trim().to_string()
        }
        _ => t.to_string(),
    }
}

/// Control del detector desde la app (en su propio hilo).
pub trait DetectorControl: Send + Sync {
    fn start(&self);
    fn stop(&self);
    fn suspend(&self);
    fn resume(&self);
    fn dismiss(&self);
    fn dismiss_in_use(&self);
}

/// El detector en un hilo que sondea cada `poll`. Los avisos se dan con el detector desbloqueado
/// (la app vuelve a llamar al detector desde ellos).
pub struct DetectorHandle {
    detector: Arc<Mutex<CallDetector>>,
    poll: Duration,
    stopping: Arc<(Mutex<bool>, Condvar)>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DetectorHandle {
    pub fn new(d: CallDetector, poll: Duration) -> Arc<DetectorHandle> {
        Arc::new(DetectorHandle {
            detector: Arc::new(Mutex::new(d)),
            poll,
            stopping: Arc::new((Mutex::new(false), Condvar::new())),
            thread: Mutex::new(None),
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut CallDetector) -> R) -> R {
        f(&mut self.detector.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl DetectorControl for DetectorHandle {
    fn start(&self) {
        let mut slot = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return;
        }
        *self.stopping.0.lock().unwrap_or_else(|e| e.into_inner()) = false;
        let (detector, stopping, poll) = (self.detector.clone(), self.stopping.clone(), self.poll);
        *slot = Some(
            std::thread::Builder::new()
                .name("detector".into())
                .spawn(move || {
                    loop {
                        let (lock, cv) = &*stopping;
                        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let (stop, _) = cv
                            .wait_timeout_while(guard, poll, |s| !*s)
                            .unwrap_or_else(|e| e.into_inner());
                        if *stop {
                            return;
                        }
                        drop(stop);
                        let run = catch_unwind(AssertUnwindSafe(|| {
                            let (events, (start, end)) = {
                                let mut d = detector.lock().unwrap_or_else(|e| e.into_inner());
                                (d.poll(), d.callbacks())
                            };
                            for e in events {
                                match e {
                                    Event::Start(det) => start(det),
                                    Event::Stop(det) => end(det),
                                }
                            }
                        }));
                        if run.is_err() {
                            log::error!("Error en el detector de llamadas");
                        }
                    }
                })
                .expect("hilo del detector"),
        );
    }

    fn stop(&self) {
        *self.stopping.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.stopping.1.notify_all();
        if let Some(t) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = t.join();
        }
    }

    fn suspend(&self) {
        self.with(CallDetector::suspend);
    }

    fn resume(&self) {
        self.with(CallDetector::resume);
    }

    fn dismiss(&self) {
        self.with(CallDetector::dismiss);
    }

    fn dismiss_in_use(&self) {
        self.with(CallDetector::dismiss_in_use);
    }
}

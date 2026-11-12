//! Coordinación (port de calltx/app.py): modo manual (atajo, bandeja) y modo automático
//! (detector). Una sola grabación a la vez; las acciones van en orden (un cerrojo reentrante, como el
//! `RLock` de Python) y el estado se lee sin esperarlas.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::{self, ThreadId};

use serde_json::Value;

use crate::config::Settings;
use crate::detector::{Detected, DetectorControl, OnDetected};
use crate::model::ModelGate;
use crate::notify::Notifier;
use crate::routing::Audio;
use crate::source::OthersSpec;
use crate::transcript::fmt_elapsed;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Armed,
    Recording,
    Preparing,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Armed => "armed",
            State::Recording => "recording",
            State::Preparing => "preparing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Note,
    Call,
    Manual,
}

pub const NOTE_LABEL: &str = "Nota";
/// Llamada grabada a mano ("Manual" ya es una etiqueta de nota antigua).
pub const CALL_LABEL: &str = "Llamada";

/// Una grabación en curso.
pub trait Recording: Send {
    /// Para, transcribe lo pendiente y cierra el archivo: (ruta, solo lo dicho).
    fn stop(self: Box<Self>) -> io::Result<(PathBuf, String)>;
    /// Marca en este momento; el segundo de la grabación, o None si no se pudo.
    fn mark(&self) -> Option<f64>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRequest {
    pub label: String,
    /// None = nota rápida (solo el micrófono).
    pub others: Option<OthersSpec>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    State(State),
    SessionStarted {
        label: String,
    },
    Line {
        speaker: String,
        hms: String,
        start_s: f64,
        text: String,
    },
    Level(f32),
    Marked {
        hms: String,
    },
    SessionStopping {
        label: String,
    },
    SessionEnded {
        path: Option<PathBuf>,
        label: String,
        copied: bool,
    },
}

pub type EventSink = Arc<dyn Fn(AppEvent) + Send + Sync>;
pub type SessionFactory =
    Box<dyn Fn(SessionRequest, EventSink) -> Result<Box<dyn Recording>, String> + Send + Sync>;
pub type Applier = Box<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;
pub type AfterApply = Box<dyn Fn(&Value) + Send + Sync>;
pub type DetectorFactory =
    Box<dyn FnOnce(OnDetected, OnDetected) -> Arc<dyn DetectorControl> + Send>;
pub type CopyText = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
pub type SaveSettings = Box<dyn Fn(&Settings) -> Result<(), String> + Send + Sync>;

pub struct AppDeps {
    pub detector: DetectorFactory,
    pub sessions: SessionFactory,
    pub notifier: Arc<dyn Notifier>,
    pub model: Arc<dyn ModelGate>,
    pub copy_text: CopyText,
    pub events: EventSink,
    pub save: SaveSettings,
}

/// Ajustes que no se cambian mientras se graba.
const BLOCKED_WHILE_RECORDING: &[&str] = &[
    "model",
    "language",
    "output_dir",
    "label_me",
    "label_others",
    "device",
];

struct Data {
    settings: Settings,
    session: Option<Box<dyn Recording>>,
    session_auto: bool,
    label: Option<String>,
    kind: Option<SessionKind>,
    /// La app se está cerrando.
    stopping: bool,
    /// Cerrando una grabación (últimas frases).
    busy: bool,
    /// Instalador lanzado: la app se cerrará en segundos.
    updating: bool,
}

pub struct App {
    action: ReLock,
    data: Mutex<Data>,
    detector: Arc<dyn DetectorControl>,
    sessions: SessionFactory,
    notifier: Arc<dyn Notifier>,
    model: Arc<dyn ModelGate>,
    copy_text: CopyText,
    events: EventSink,
    save: SaveSettings,
    appliers: Mutex<HashMap<String, Applier>>,
    after_apply: Mutex<HashMap<String, AfterApply>>,
}

impl App {
    pub fn new(settings: Settings, deps: AppDeps) -> Arc<App> {
        Arc::new_cyclic(|me: &Weak<App>| {
            let (a, b) = (me.clone(), me.clone());
            let on_start: OnDetected = Box::new(move |d| {
                if let Some(app) = a.upgrade() {
                    app.on_call_start(d);
                }
            });
            let on_stop: OnDetected = Box::new(move |d| {
                if let Some(app) = b.upgrade() {
                    app.on_call_stop(d);
                }
            });
            App {
                action: ReLock::default(),
                data: Mutex::new(Data {
                    settings,
                    session: None,
                    session_auto: false,
                    label: None,
                    kind: None,
                    stopping: false,
                    busy: false,
                    updating: false,
                }),
                detector: (deps.detector)(on_start, on_stop),
                sessions: deps.sessions,
                notifier: deps.notifier,
                model: deps.model,
                copy_text: deps.copy_text,
                events: deps.events,
                save: deps.save,
                appliers: Mutex::new(HashMap::new()),
                after_apply: Mutex::new(HashMap::new()),
            }
        })
    }

    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, e: AppEvent) {
        (self.events)(e);
    }

    fn notify(&self, title: &str, body: &str) {
        self.notifier.notify(title, body, None);
    }

    // --- estado --------------------------------------------------------------

    pub fn state(&self) -> State {
        let (recording, auto) = {
            let d = self.data();
            (d.session.is_some(), d.settings.auto_mode)
        };
        if recording {
            State::Recording
        } else if self.model.preparing().is_some() {
            State::Preparing
        } else if auto {
            State::Armed
        } else {
            State::Idle
        }
    }

    pub fn session_kind(&self) -> Option<SessionKind> {
        self.data().kind
    }

    /// Grabando o terminando de guardar la última grabación (no cerrar la app).
    pub fn recording_active(&self) -> bool {
        let d = self.data();
        d.session.is_some() || d.busy
    }

    pub fn settings(&self) -> Settings {
        self.data().settings.clone()
    }

    pub fn set_updating(&self, on: bool) {
        self.data().updating = on;
    }

    /// El modelo cambió de estado (preparándose ↔ listo): se vuelve a avisar del estado.
    pub fn model_changed(&self) {
        self.emit(AppEvent::State(self.state()));
    }

    fn changed(&self) {
        self.emit(AppEvent::State(self.state()));
    }

    fn sync_detector(&self) {
        let resume = {
            let d = self.data();
            d.settings.auto_mode && !(d.session.is_some() && !d.session_auto)
        };
        if resume {
            self.detector.resume();
        } else {
            self.detector.suspend();
        }
    }

    // --- ciclo de vida -------------------------------------------------------

    pub fn start(&self) {
        self.sync_detector();
        self.detector.start();
        self.changed();
    }

    pub fn quit(&self) {
        self.data().stopping = true;
        self.detector.stop(); // fuera del cerrojo: el detector puede estar esperándolo
        let _g = self.action.lock();
        if self.data().session.is_some() {
            self.stop_session(true);
        }
    }

    // --- acciones ------------------------------------------------------------

    /// Atajo y "Nueva nota rápida": para cualquier grabación; si no hay, empieza una nota.
    pub fn toggle(&self) {
        {
            let _g = self.action.lock();
            if self.data().session.is_some() {
                self.stop_session(true);
                return;
            }
        }
        self.start_manual();
    }

    pub fn toggle_call(&self) {
        {
            let _g = self.action.lock();
            if self.data().session.is_some() {
                self.stop_session(true);
                return;
            }
        }
        self.start_call_manual();
    }

    pub fn start_manual(&self) {
        if self.data().stopping || !self.model_ready() {
            return;
        }
        let _g = self.action.lock();
        if self.can_begin() {
            self.begin(NOTE_LABEL, None, false, None);
        }
    }

    /// Grabar llamada a mano: micrófono + lo que suena en la salida de la llamada.
    pub fn start_call_manual(&self) {
        if self.data().stopping || !self.model_ready() {
            return;
        }
        let _g = self.action.lock();
        if self.can_begin() {
            let others = OthersSpec {
                kind: Audio::Device,
                exe: None,
                platform: None,
            };
            self.begin(CALL_LABEL, Some(others), false, None);
        }
    }

    fn can_begin(&self) -> bool {
        let d = self.data();
        d.session.is_none() && !d.stopping
    }

    pub fn set_auto(&self, on: bool) {
        {
            let _g = self.action.lock();
            self.data().settings.auto_mode = on;
            self.sync_detector();
        }
        self.changed();
    }

    /// Marca durante una llamada (automática o a mano). false si no hay llamada.
    pub fn mark(&self) -> bool {
        let at = {
            let d = self.data();
            match (&d.session, d.kind) {
                (Some(s), Some(SessionKind::Call | SessionKind::Manual)) => s.mark(),
                _ => None,
            }
        };
        match at {
            Some(t) => {
                self.emit(AppEvent::Marked {
                    hms: fmt_elapsed(t),
                });
                true
            }
            None => false,
        }
    }

    pub fn on_call_start(&self, d: Detected) {
        if self.data().stopping || !self.model_ready() {
            return;
        }
        let _g = self.action.lock();
        if !self.can_begin() {
            return;
        }
        let p = d.platform;
        let others = match p.audio {
            Audio::Device => OthersSpec {
                kind: Audio::Device,
                exe: None,
                platform: Some(p.key.into()),
            },
            Audio::Process => OthersSpec {
                kind: Audio::Process,
                exe: d.exe.clone(),
                platform: Some(p.key.into()),
            },
        };
        self.begin(&d.label, Some(others), true, d.title.clone());
    }

    pub fn on_call_stop(&self, _d: Detected) {
        let _g = self.action.lock();
        let auto = {
            let d = self.data();
            d.session.is_some() && d.session_auto
        };
        if auto {
            self.stop_session(false);
        }
    }

    /// No bloquea: mientras se prepara el modelo (descargas del primer uso) no se graba.
    fn model_ready(&self) -> bool {
        if self.data().updating {
            self.notify(
                "Actualizando la app",
                "Podrás grabar en cuanto se vuelva a abrir",
            );
            return false;
        }
        let prep = self.model.preparing();
        if prep.is_none() && self.model.ready() {
            return true;
        }
        match prep {
            None => {
                // No hay modelo y no se está preparando (p. ej. falló la descarga sin conexión).
                self.notify(
                    "Modelo no disponible",
                    "Reintentando la descarga; podrás grabar en cuanto termine",
                );
                self.model.request_reload();
            }
            Some((p, _)) => self.notify(
                "Aún preparando el modelo",
                &format!("Podrás grabar en cuanto termine ({} %)", (p * 100.0) as i32),
            ),
        }
        false
    }

    // --- interno (con el cerrojo de acciones) ----------------------------------

    fn begin(&self, label: &str, others: Option<OthersSpec>, auto: bool, title: Option<String>) {
        let kind = match (&others, auto) {
            (None, _) => SessionKind::Note,
            (Some(_), true) => SessionKind::Call,
            (Some(_), false) => SessionKind::Manual,
        };
        let req = SessionRequest {
            label: label.to_string(),
            others,
            title,
        };
        let session = match (self.sessions)(req, self.events.clone()) {
            Ok(s) => s,
            Err(e) => {
                log::error!("No se pudo iniciar la grabación: {e}");
                self.notify("Error al grabar", &e);
                return;
            }
        };
        {
            let mut d = self.data();
            d.session = Some(session);
            d.session_auto = auto;
            d.label = Some(label.to_string());
            d.kind = Some(kind);
        }
        self.sync_detector();
        self.emit(AppEvent::SessionStarted {
            label: label.to_string(),
        });
        if label != NOTE_LABEL {
            self.notify(
                "Grabando llamada",
                &format!("{label} — transcripción en curso"),
            );
        }
        self.changed();
    }

    fn stop_session(&self, user: bool) {
        let (session, was_auto, label, kind, copy_on_stop) = {
            let mut d = self.data();
            let Some(session) = d.session.take() else {
                return;
            };
            d.busy = true;
            let was_auto = std::mem::take(&mut d.session_auto);
            (
                session,
                was_auto,
                d.label.take().unwrap_or_default(),
                d.kind.take(),
                d.settings.copy_on_stop,
            )
        };
        self.emit(AppEvent::SessionStopping {
            label: label.clone(),
        });
        let (mut path, mut copied) = (None, false);
        match session.stop() {
            Ok((p, text)) => {
                if label == NOTE_LABEL {
                    copied = copy_on_stop && self.copy_note(&text);
                } else {
                    self.notifier.notify(
                        "Transcripción guardada",
                        &p.display().to_string(),
                        Some(&p),
                    );
                }
                path = Some(p);
            }
            Err(e) => {
                log::error!("Error cerrando la grabación: {e}");
                self.notify("Error al guardar", &e.to_string());
            }
        }
        self.data().busy = false;
        self.emit(AppEvent::SessionEnded {
            path,
            label,
            copied,
        });
        if was_auto && user {
            self.detector.dismiss();
        } else if kind == Some(SessionKind::Manual) && user {
            self.detector.dismiss_in_use();
        }
        self.sync_detector();
        self.changed();
    }

    /// Las notas rápidas se copian al portapapeles (solo el texto dicho). Las llamadas nunca.
    fn copy_note(&self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        match (self.copy_text)(text) {
            Ok(()) => true,
            Err(e) => {
                log::error!("No se pudo copiar la nota al portapapeles: {e}");
                false
            }
        }
    }

    // --- ajustes desde la ventana --------------------------------------------

    pub fn set_applier(&self, key: &str, f: Applier) {
        self.appliers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.into(), f);
    }

    pub fn set_after_apply(&self, key: &str, f: AfterApply) {
        self.after_apply
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.into(), f);
    }

    /// Aplica un ajuste en vivo y lo guarda. Err con el mensaje para la ventana.
    pub fn apply_setting(&self, key: &str, value: Value) -> Result<(), String> {
        // ¿Existe y el valor es de su tipo? (sobre una copia)
        self.settings().set_field(key, value.clone())?;
        {
            let _g = self.action.lock();
            {
                let d = self.data();
                if BLOCKED_WHILE_RECORDING.contains(&key) && (d.session.is_some() || d.busy) {
                    return Err("No se puede cambiar mientras se graba".into());
                }
            }
            if key == "auto_mode" {
                self.set_auto(value.as_bool().unwrap_or(true));
            } else {
                if let Some(hook) = self
                    .appliers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(key)
                {
                    hook(&value).map_err(|e| {
                        if e.is_empty() {
                            "No se pudo aplicar el ajuste".to_string()
                        } else {
                            e
                        }
                    })?;
                }
                self.data().settings.set_field(key, value.clone())?;
            }
        }
        if let Err(e) = (self.save)(&self.settings()) {
            log::error!("No se pudieron guardar los ajustes: {e}");
        }
        if let Some(after) = self
            .after_apply
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
        {
            after(&value);
        }
        self.changed();
        Ok(())
    }
}

/// Cerrojo reentrante (como el `RLock` de Python): el mismo hilo puede volver a tomarlo, p. ej. un
/// aviso de "parando" que aplica un ajuste mientras se cierra la grabación.
#[derive(Default)]
struct ReLock {
    owner: Mutex<(Option<ThreadId>, usize)>,
    released: Condvar,
}

struct ReGuard<'a>(&'a ReLock);

impl ReLock {
    fn lock(&self) -> ReGuard<'_> {
        let me = thread::current().id();
        let mut o = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match o.0 {
                None => {
                    *o = (Some(me), 1);
                    break;
                }
                Some(t) if t == me => {
                    o.1 += 1;
                    break;
                }
                Some(_) => o = self.released.wait(o).unwrap_or_else(|e| e.into_inner()),
            }
        }
        ReGuard(self)
    }
}

impl Drop for ReGuard<'_> {
    fn drop(&mut self) {
        let mut o = self.0.owner.lock().unwrap_or_else(|e| e.into_inner());
        o.1 -= 1;
        if o.1 == 0 {
            o.0 = None;
            self.0.released.notify_all();
        }
    }
}

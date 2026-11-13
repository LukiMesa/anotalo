//! Las órdenes de la ventana (port de calltx/ui/api.py): mismos nombres, argumentos y JSON. Todo
//! devuelve JSON y nada falla hacia la página: los errores son `{"ok": false, "error": "…"}`. Lo
//! que toca Windows (portapapeles, diálogos, la ventana, pausar los atajos) llega en `UiDeps`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use serde_json::{Map, Value, json};

use crate::app::{App, CALL_LABEL, CopyText, SessionKind};
use crate::library::{Kind, Recording, Trash, fold, list_recordings, parse_recording, to_readable};
use crate::meta::{set_client, validate_client};
use crate::paths::{Profile, profile};
use crate::speaker::memory::VoiceStore;
use crate::speaker::naming::{forget_in_call_prints, is_reserved, name_in_call, validate_name};

const MONTHS: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
];
/// Los ajustes que la ventana puede cambiar (los de Python más `hotkey_mark`), en orden.
pub const EDITABLE: [&str; 18] = [
    "auto_mode",
    "autostart",
    "copy_on_stop",
    "detect_browsers",
    "detect_discord",
    "detect_slack",
    "detect_teams",
    "detect_whatsapp",
    "detect_zoom",
    "hotkey",
    "hotkey_mark",
    "label_me",
    "label_others",
    "language",
    "model",
    "output_dir",
    "release_after_min",
    "show_indicator",
];
/// Minutos sin grabar hasta liberar el modelo (0 = nunca).
pub const RELEASE_OPTIONS: [i64; 4] = [0, 2, 10, 30];
/// Si la ventana no avisa del fin de la captura de un atajo, los atajos vuelven solos.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

const GONE: &str = "La grabación ya no existe";
const INSTALLED_ONLY: &str = "Solo en la versión instalada";
const LOG_JS_CHARS: usize = 2000;

/// "Guardar como" con el nombre propuesto; None si se cancela.
pub type SaveDialog = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// La ventana, para los botones de su barra de título.
pub trait WindowControl: Send + Sync {
    fn minimize(&self);
    /// Maximiza o restaura; true si queda maximizada.
    fn toggle_maximize(&self) -> bool;
    fn hide(&self);
}

pub struct UiDeps {
    pub app: Arc<App>,
    /// El nombre del archivo que se está grabando, si hay grabación.
    pub live_id: Box<dyn Fn() -> Option<String> + Send + Sync>,
    pub copy: CopyText,
    pub save_dialog: SaveDialog,
    /// Elegir carpeta; None si se cancela.
    pub folder_dialog: Box<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    pub today: Box<dyn Fn() -> NaiveDate + Send + Sync>,
    pub device_info: Box<dyn Fn() -> String + Send + Sync>,
    /// true = soltar los atajos globales mientras la ventana escucha uno nuevo; false = volver.
    pub hotkey_capture: Box<dyn Fn(bool) + Send + Sync>,
    pub window: Arc<dyn WindowControl>,
    pub voices_path: PathBuf,
    pub prints_dir: PathBuf,
    /// Lo que va detrás de la versión y los ajustes en "Copiar diagnóstico" (audio, registro…).
    pub diagnostics: Box<dyn Fn() -> String + Send + Sync>,
    pub version: String,
}

pub struct UiApi {
    app: Arc<App>,
    live_id: Box<dyn Fn() -> Option<String> + Send + Sync>,
    copy_text: CopyText,
    save_dialog: SaveDialog,
    folder_dialog: Box<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    today: Box<dyn Fn() -> NaiveDate + Send + Sync>,
    device_info: Box<dyn Fn() -> String + Send + Sync>,
    window: Arc<dyn WindowControl>,
    voices_path: PathBuf,
    prints_dir: PathBuf,
    diagnostics: Box<dyn Fn() -> String + Send + Sync>,
    version: String,
    capture: Arc<Capture>,
    /// Los cambios en las voces guardadas van de uno en uno (cargar, cambiar, guardar).
    voices: Mutex<()>,
}

fn ok() -> Value {
    json!({"ok": true})
}

fn err(msg: impl Into<String>) -> Value {
    json!({"ok": false, "error": msg.into()})
}

fn cancelled() -> Value {
    json!({"ok": false, "cancelled": true})
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `m:ss` o `h:mm:ss`; "" sin duración.
fn duration_label(seconds: Option<u32>) -> String {
    let Some(s) = seconds else {
        return String::new();
    };
    let (h, rem) = (s / 3600, s % 3600);
    if h > 0 {
        format!("{h}:{:02}:{:02}", rem / 60, rem % 60)
    } else {
        format!("{:02}:{:02}", rem / 60, rem % 60)
    }
}

/// El inicio, si es una fecha de verdad (como en Python, desde 1900).
fn dated(rec: &Recording) -> Option<NaiveDateTime> {
    rec.started_at.filter(|d| d.year() >= 1900)
}

fn kind_label(k: SessionKind) -> &'static str {
    match k {
        SessionKind::Note => "note",
        SessionKind::Call => "call",
        SessionKind::Manual => "manual",
    }
}

/// `int(x)` de Python para lo que pueda mandar la página (None si no es un entero).
fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        }),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Un nombre de archivo de la carpeta (sin rutas, sin `:`), terminado en `.md`.
fn plain_md_name(id: &str) -> bool {
    !id.is_empty()
        && Path::new(id).file_name().and_then(|n| n.to_str()) == Some(id)
        && !id.contains([':', '/', '\\'])
        && id.ends_with(".md")
}

impl UiApi {
    pub fn new(deps: UiDeps) -> UiApi {
        let UiDeps {
            app,
            live_id,
            copy,
            save_dialog,
            folder_dialog,
            today,
            device_info,
            hotkey_capture,
            window,
            voices_path,
            prints_dir,
            diagnostics,
            version,
        } = deps;
        UiApi {
            app,
            live_id,
            copy_text: copy,
            save_dialog,
            folder_dialog,
            today,
            device_info,
            window,
            voices_path,
            prints_dir,
            diagnostics,
            version,
            capture: Arc::new(Capture::new(hotkey_capture)),
            voices: Mutex::new(()),
        }
    }

    /// Cuánto espera la captura de un atajo antes de devolver los atajos ([`CAPTURE_TIMEOUT`]).
    pub fn set_capture_timeout(&self, timeout: Duration) {
        *lock(&self.capture.timeout) = timeout;
    }

    /// Una orden de la página por su nombre, con sus argumentos tal como llegan (los que faltan o
    /// no son del tipo esperado cuentan como vacíos o su valor por defecto).
    pub fn call(&self, method: &str, args: &[Value]) -> Value {
        let s = |i: usize| args.get(i).and_then(Value::as_str).unwrap_or_default();
        match method {
            "list_recordings" => {
                self.list_recordings(s(0), s(1), args.get(2).and_then(Value::as_str))
            }
            "get_recording" => self.get_recording(s(0)),
            "copy" => self.copy(s(0)),
            "save_as" => self.save_as(s(0)),
            "delete" => self.delete(s(0)),
            "undo" => self.undo(s(0)),
            "open_folder" => self.open_folder(),
            "get_state" => self.get_state(),
            "check_updates" => self.check_updates(),
            "install_update" => self.install_update(),
            "toggle_call" => self.toggle_call(),
            "get_settings" => self.get_settings(),
            "set_setting" => self.set_setting(s(0), args.get(1).cloned().unwrap_or_default()),
            "set_hotkey" => self.set_hotkey(s(0)),
            "set_hotkey_mark" => self.set_hotkey_mark(s(0)),
            "begin_hotkey_capture" => self.begin_hotkey_capture(),
            "end_hotkey_capture" => self.end_hotkey_capture(),
            "purge_trash" => self.purge_trash(args.first().and_then(Value::as_f64).unwrap_or(5.0)),
            "log_js" => {
                let message = match args.first() {
                    Some(Value::String(m)) => m.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                self.log_js(&message)
            }
            "window_minimize" => self.window_minimize(),
            "window_toggle_maximize" => self.window_toggle_maximize(),
            "window_close" => self.window_close(),
            "choose_folder" => self.choose_folder(),
            "set_client" => self.set_client(s(0), s(1)),
            "rename_person" => self.rename_person(s(0), s(1), s(2)),
            "list_voices" => self.list_voices(),
            "rename_voice" => self.rename_voice(s(0), s(1)),
            "forget_voice" => self.forget_voice(s(0)),
            "copy_diagnostics" => self.copy_diagnostics(),
            _ => err(format!("Orden desconocida: {method}")),
        }
    }

    // --- utilidades -------------------------------------------------------------

    fn folder(&self) -> PathBuf {
        PathBuf::from(self.app.settings().output_dir)
    }

    /// La ruta de una grabación de la carpeta, o None si el id no es un nombre de archivo o ya no
    /// existe.
    fn path(&self, id: &str) -> Option<PathBuf> {
        if !plain_md_name(id) {
            return None;
        }
        let p = self.folder().join(id);
        p.is_file().then_some(p)
    }

    fn is_live(&self, id: &str) -> bool {
        (self.live_id)().as_deref() == Some(id)
    }

    fn date_label(&self, rec: &Recording) -> String {
        let Some(start) = dated(rec) else {
            return "Sin fecha".into();
        };
        let (d, today) = (start.date(), (self.today)());
        if d == today {
            return "Hoy".into();
        }
        if Some(d) == today.pred_opt() {
            return "Ayer".into();
        }
        let label = format!("{} {}", d.day(), MONTHS[d.month0() as usize]);
        if d.year() == today.year() {
            label
        } else {
            format!("{label} {}", d.year())
        }
    }

    fn summary(&self, rec: &Recording, live: Option<&str>, label_me: &str) -> Map<String, Value> {
        let app = match rec.kind {
            // Las llamadas grabadas a mano se guardan como "Llamada": se muestran como "Manual".
            Kind::Call if rec.app == CALL_LABEL => "Manual",
            Kind::Call => rec.app.as_str(),
            Kind::Note => "",
        };
        let title = match rec.kind {
            Kind::Note => "Nota rápida",
            Kind::Call => "Llamada",
        };
        let time = dated(rec).map_or_else(String::new, |d| d.format("%H:%M").to_string());
        let mut m = Map::new();
        m.insert("id".into(), rec.id.clone().into());
        m.insert("kind".into(), rec.kind.as_str().into());
        m.insert("app".into(), app.into());
        m.insert("title".into(), title.into());
        m.insert("date_label".into(), self.date_label(rec).into());
        m.insert("time".into(), time.into());
        m.insert("duration".into(), duration_label(rec.duration_s).into());
        m.insert("snippet".into(), rec.snippet(label_me).into());
        m.insert("live".into(), (live == Some(rec.id.as_str())).into());
        m.insert("call_title".into(), rec.title.clone().into());
        m.insert("client".into(), rec.client.clone().into());
        m
    }

    // --- grabaciones -------------------------------------------------------------

    /// La lista (más reciente primero). `client` filtra por cliente; nunca falla: [] si algo va mal.
    pub fn list_recordings(&self, kind: &str, query: &str, client: Option<&str>) -> Value {
        let settings = self.app.settings();
        let live = (self.live_id)();
        let recs = list_recordings(Path::new(&settings.output_dir), kind, query, client);
        Value::Array(
            recs.iter()
                .map(|r| Value::Object(self.summary(r, live.as_deref(), &settings.label_me)))
                .collect(),
        )
    }

    pub fn get_recording(&self, id: &str) -> Value {
        let Some(p) = self.path(id) else {
            return err(GONE);
        };
        let rec = parse_recording(&p);
        let me = self.app.settings().label_me;
        let mut out = self.summary(&rec, (self.live_id)().as_deref(), &me);
        let start = dated(&rec);
        let hm = |d: NaiveDateTime| d.format("%H:%M").to_string();
        let (start_hm, end_hm) = (
            start.map(hm).unwrap_or_default(),
            rec.ended_at.map(hm).unwrap_or_default(),
        );
        let full_date = start.map_or_else(String::new, |d| {
            format!("{} {} {}", d.day(), MONTHS[d.month0() as usize], d.year())
        });
        let start_end = if end_hm.is_empty() {
            start_hm
        } else {
            format!("{start_hm} – {end_hm}")
        };
        let lines: Vec<Value> = rec
            .lines
            .iter()
            .map(|l| {
                json!({
                    "ts": l.ts,
                    "speaker": l.speaker,
                    "me": !l.mark && l.speaker == me,
                    "text": l.text,
                    "mark": l.mark,
                })
            })
            .collect();
        out.insert("ok".into(), true.into());
        out.insert("full_date".into(), full_date.into());
        out.insert("start_end".into(), start_end.into());
        out.insert("people".into(), rec.people(&me).into());
        out.insert("lines".into(), lines.into());
        Value::Object(out)
    }

    pub fn copy(&self, id: &str) -> Value {
        let Some(p) = self.path(id) else {
            return err(GONE);
        };
        match (self.copy_text)(&to_readable(&parse_recording(&p))) {
            Ok(()) => ok(),
            Err(e) => err(format!("No se pudo copiar: {e}")),
        }
    }

    pub fn save_as(&self, id: &str) -> Value {
        let Some(p) = self.path(id) else {
            return err(GONE);
        };
        let Some(dest) = (self.save_dialog)(id) else {
            return cancelled();
        };
        let same = matches!(
            (fs::canonicalize(&p), fs::canonicalize(&dest)),
            (Ok(a), Ok(b)) if a == b
        );
        if same {
            return err("No se pudo guardar: es el mismo archivo");
        }
        match fs::copy(&p, &dest) {
            Ok(_) => json!({"ok": true, "path": dest.to_string_lossy()}),
            Err(e) => err(format!("No se pudo guardar: {e}")),
        }
    }

    pub fn delete(&self, id: &str) -> Value {
        let Some(p) = self.path(id) else {
            return err(GONE);
        };
        if self.is_live(id) {
            return err("No se puede borrar mientras se está grabando");
        }
        match Trash::new(&self.folder()).delete(&p) {
            Ok(token) => json!({"ok": true, "token": token}),
            Err(e) => err(format!("No se pudo borrar: {e}")),
        }
    }

    pub fn undo(&self, token: &str) -> Value {
        match Trash::new(&self.folder()).undo(token) {
            Ok(_) => ok(),
            Err(e) => err(format!("No se pudo deshacer: {e}")),
        }
    }

    pub fn open_folder(&self) -> Value {
        let dir = self.folder();
        let opened = fs::create_dir_all(&dir).and_then(|()| {
            std::process::Command::new("explorer.exe")
                .arg(&dir)
                .spawn()
                .map(drop)
        });
        match opened {
            Ok(()) => ok(),
            Err(e) => err(e.to_string()),
        }
    }

    /// Borra de verdad lo que lleva en la papelera más de `older_than_s` segundos.
    pub fn purge_trash(&self, older_than_s: f64) -> Value {
        match Trash::new(&self.folder()).purge(older_than_s) {
            Ok(n) => json!({"ok": true, "purged": n}),
            Err(e) => err(e.to_string()),
        }
    }

    /// Pone (o quita, con "") el cliente de una grabación. Devuelve el cliente tal como quedó.
    pub fn set_client(&self, id: &str, client: &str) -> Value {
        let Some(p) = self.path(id) else {
            return err(GONE);
        };
        if self.is_live(id) {
            return err("La llamada aún se está grabando; pon el cliente al colgar");
        }
        match set_client(&p, client) {
            Ok(()) => {
                let c = validate_client(client).unwrap_or_default();
                json!({"ok": true, "client": (!c.is_empty()).then_some(c)})
            }
            Err(e) => err(e),
        }
    }

    /// Nombra a una persona de la llamada (spec §2.4.5) y, si la llamada guardó su huella, la
    /// recuerda. Las etiquetas de la app (`label_me`, `label_others`, "Persona N") corrigen el
    /// archivo pero no son un nombre: `remembered` es false.
    pub fn rename_person(&self, id: &str, from: &str, to: &str) -> Value {
        let Some(p) = self.path(id) else {
            return err(GONE);
        };
        if self.is_live(id) {
            return err("La llamada aún se está grabando; ponle nombre al colgar");
        }
        let s = self.app.settings();
        let reserved = [s.label_me.as_str(), s.label_others.as_str()];
        let _one = lock(&self.voices);
        let mut store = VoiceStore::load(&self.voices_path);
        match name_in_call(&p, &self.prints_dir, &mut store, from, to, &reserved) {
            Ok(o) => json!({"ok": true, "lines": o.lines, "remembered": o.remembered}),
            Err(e) => err(e),
        }
    }

    // --- voces guardadas -----------------------------------------------------------

    /// `[{name, minutes}]` por orden alfabético (minutos de voz con un decimal).
    pub fn list_voices(&self) -> Value {
        let _one = lock(&self.voices);
        let store = VoiceStore::load(&self.voices_path);
        let mut voices: Vec<(String, &str, f64)> = store
            .voices
            .iter()
            .map(|v| (fold(&v.name), v.name.as_str(), v.seconds))
            .collect();
        voices.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        Value::Array(
            voices
                .into_iter()
                .map(|(_, name, s)| json!({"name": name, "minutes": (s / 6.0).round() / 10.0}))
                .collect(),
        )
    }

    pub fn rename_voice(&self, from: &str, to: &str) -> Value {
        let to = match validate_name(to) {
            Ok(t) => t,
            Err(e) => return err(e),
        };
        let s = self.app.settings();
        if is_reserved(&to, &[&s.label_me, &s.label_others]) {
            return err(format!(
                "Nombre no válido: «{to}» es una etiqueta de la app"
            ));
        }
        let _one = lock(&self.voices);
        let mut store = VoiceStore::load(&self.voices_path);
        if let Err(e) = store.rename(from, &to) {
            return err(e);
        }
        match store.save() {
            Ok(()) => ok(),
            Err(e) => err(format!("No se pudo guardar la voz: {e}")),
        }
    }

    /// Borra la voz guardada y su huella de las voces de cada llamada (si no, se podría volver a
    /// recordar desde ellas).
    pub fn forget_voice(&self, name: &str) -> Value {
        let _one = lock(&self.voices);
        let mut store = VoiceStore::load(&self.voices_path);
        if store.forget(name)
            && let Err(e) = store.save()
        {
            return err(format!("No se pudo borrar la voz: {e}"));
        }
        let n = forget_in_call_prints(&self.prints_dir, name);
        if n > 0 {
            log::info!("Voz borrada también de {n} llamadas");
        }
        ok()
    }

    // --- estado y ajustes -----------------------------------------------------------

    pub fn get_state(&self) -> Value {
        json!({
            "state": self.app.state().as_str(),
            "hotkey": self.app.settings().hotkey,
            "device": (self.device_info)(),
            "version": self.version,
            "frozen": profile() == Profile::Installed,
            "kind": self.app.session_kind().map(kind_label),
            "update": Value::Null,
        })
    }

    /// Las actualizaciones llegan con el instalador (M4).
    pub fn check_updates(&self) -> Value {
        err(INSTALLED_ONLY)
    }

    pub fn install_update(&self) -> Value {
        err(INSTALLED_ONLY)
    }

    /// Botón "Grabar llamada": empieza una llamada manual o para lo que se esté grabando.
    pub fn toggle_call(&self) -> Value {
        self.app.toggle_call();
        ok()
    }

    pub fn get_settings(&self) -> Value {
        let all = serde_json::to_value(self.app.settings()).unwrap_or_default();
        let mut out = Map::new();
        for key in EDITABLE {
            out.insert(key.into(), all.get(key).cloned().unwrap_or_default());
        }
        Value::Object(out)
    }

    pub fn set_setting(&self, key: &str, value: Value) -> Value {
        if !EDITABLE.contains(&key) {
            return err(format!("Ajuste desconocido: {key}"));
        }
        let value = if key == "release_after_min" {
            match py_int(&value).filter(|v| RELEASE_OPTIONS.contains(v)) {
                Some(v) => Value::from(v),
                None => return err("Valor no válido"),
            }
        } else {
            value
        };
        match self.app.apply_setting(key, value) {
            Ok(()) => ok(),
            Err(e) => {
                log::warn!("Ajuste {key} no aplicado: {e}");
                err(e)
            }
        }
    }

    pub fn set_hotkey(&self, spec: &str) -> Value {
        self.set_setting("hotkey", spec.into())
    }

    pub fn set_hotkey_mark(&self, spec: &str) -> Value {
        self.set_setting("hotkey_mark", spec.into())
    }

    /// Mientras la ventana escucha un atajo nuevo, los globales se sueltan para poder pulsarlo.
    /// Por seguridad vuelven solos tras [`CAPTURE_TIMEOUT`] aunque la ventana no avise.
    pub fn begin_hotkey_capture(&self) -> Value {
        self.capture.begin();
        ok()
    }

    /// Devuelve los atajos (una sola vez aunque se llame varias).
    pub fn end_hotkey_capture(&self) -> Value {
        self.capture.end();
        ok()
    }

    pub fn log_js(&self, message: &str) -> Value {
        let message: String = message.chars().take(LOG_JS_CHARS).collect();
        log::warn!("Ventana: {message}");
        ok()
    }

    /// Versión, ajustes (sin las claves desconocidas) y lo que dé `UiDeps::diagnostics`, al
    /// portapapeles.
    pub fn copy_diagnostics(&self) -> Value {
        match (self.copy_text)(&self.diagnostics_text()) {
            Ok(()) => ok(),
            Err(e) => err(format!("No se pudo copiar: {e}")),
        }
    }

    fn diagnostics_text(&self) -> String {
        let mut settings = self.app.settings();
        settings.extra.clear();
        let settings = serde_json::to_string_pretty(&settings).unwrap_or_default();
        let profile = match profile() {
            Profile::Installed => "instalada",
            Profile::Dev => "desarrollo",
        };
        format!(
            "## Versión\nAnotalo {} ({profile})\n\n## Ajustes\n{settings}\n\n{}",
            self.version,
            (self.diagnostics)()
        )
    }

    // --- barra propia de la ventana -------------------------------------------------

    pub fn window_minimize(&self) -> Value {
        self.window.minimize();
        ok()
    }

    pub fn window_toggle_maximize(&self) -> Value {
        json!({"ok": true, "maximized": self.window.toggle_maximize()})
    }

    /// Cerrar oculta la ventana (y termina una captura de atajo a medias).
    pub fn window_close(&self) -> Value {
        self.capture.end();
        self.window.hide();
        ok()
    }

    pub fn choose_folder(&self) -> Value {
        let Some(p) = (self.folder_dialog)() else {
            return cancelled();
        };
        let path = p.to_string_lossy().into_owned();
        let mut out = self.set_setting("output_dir", path.clone().into());
        out["path"] = path.into();
        out
    }
}

/// La captura de un atajo: los atajos globales en pausa y el plazo para devolverlos.
struct Capture {
    hook: Box<dyn Fn(bool) + Send + Sync>,
    state: Mutex<CaptureState>,
    changed: Condvar,
    timeout: Mutex<Duration>,
}

#[derive(Default)]
struct CaptureState {
    /// Los atajos están en pausa.
    on: bool,
    /// Sube con cada inicio y cada fin: el plazo de una captura anterior ya no cuenta.
    generation: u64,
}

impl Capture {
    fn new(hook: Box<dyn Fn(bool) + Send + Sync>) -> Capture {
        Capture {
            hook,
            state: Mutex::new(CaptureState::default()),
            changed: Condvar::new(),
            timeout: Mutex::new(CAPTURE_TIMEOUT),
        }
    }

    fn begin(self: &Arc<Self>) {
        let generation = {
            let mut st = lock(&self.state);
            if !st.on {
                (self.hook)(true);
                st.on = true;
            }
            st.generation += 1;
            self.changed.notify_all();
            st.generation
        };
        let timeout = *lock(&self.timeout);
        let me = Arc::clone(self);
        let timer = thread::Builder::new()
            .name("captura-atajo".into())
            .spawn(move || me.expire(generation, timeout));
        if let Err(e) = timer {
            // sin plazo no se dejan los atajos en pausa
            log::error!("No se pudo vigilar la captura del atajo: {e}");
            self.end();
        }
    }

    fn expire(&self, generation: u64, timeout: Duration) {
        let st = lock(&self.state);
        let (mut st, _) = self
            .changed
            .wait_timeout_while(st, timeout, |s| s.generation == generation)
            .unwrap_or_else(|e| e.into_inner());
        if st.generation == generation {
            log::info!("Captura del atajo sin terminar: se devuelven los atajos");
            self.finish(&mut st);
        }
    }

    fn end(&self) {
        let mut st = lock(&self.state);
        self.finish(&mut st);
    }

    fn finish(&self, st: &mut CaptureState) {
        st.generation += 1;
        self.changed.notify_all();
        if st.on {
            st.on = false;
            (self.hook)(false);
        }
    }
}

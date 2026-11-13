//! Ajustes del usuario en `%APPDATA%\Anotalo\config.json`, con las mismas claves que la app de
//! Python (calltx/config.py). Las claves que ya no hacen nada (`compute_type`, `save_audio`) y las
//! desconocidas se leen y se vuelven a escribir tal cual; las nuevas de Anotalo van al final.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::paths::{Profile, data_dir, data_dir_in, default_output_dir, legacy_data_dir_in};

/// Llama a `$cb!` con todos los campos conocidos de `Settings`, en orden.
macro_rules! with_settings_fields {
    ($cb:ident) => {
        $cb!(
            model,
            language,
            device,
            compute_type,
            label_me,
            label_others,
            hotkey,
            auto_mode,
            output_dir,
            mic_device,
            silence_ms,
            max_segment_s,
            min_segment_ms,
            save_audio,
            copy_on_stop,
            show_indicator,
            autostart,
            autostart_configured,
            detect_teams,
            detect_zoom,
            detect_whatsapp,
            detect_slack,
            detect_discord,
            detect_browsers,
            release_after_min,
            update_notified,
            last_run_version,
            hotkey_mark,
            igpu_faster,
            igpu_name,
            auto_update_check,
            claude_instruction,
            window_rect,
        )
    };
}

/// La instrucción de "Copiar para Claude" si el usuario no pone otra.
pub const DEFAULT_CLAUDE_INSTRUCTION: &str =
    "Saca los acuerdos, tareas y pendientes de esta llamada y propón cómo ejecutarlos";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    /// auto = large-v3-turbo con GPU, small con procesador.
    pub model: String,
    pub language: String,
    /// auto | gpu | cpu (el antiguo "cuda" pasa a "auto").
    pub device: String,
    pub compute_type: String,
    pub label_me: String,
    pub label_others: String,
    pub hotkey: String,
    pub auto_mode: bool,
    pub output_dir: String,
    pub mic_device: Option<String>,
    pub silence_ms: u32,
    pub max_segment_s: f64,
    pub min_segment_ms: u32,
    pub save_audio: bool,
    pub copy_on_stop: bool,
    pub show_indicator: bool,
    pub autostart: bool,
    pub autostart_configured: bool,
    pub detect_teams: bool,
    pub detect_zoom: bool,
    pub detect_whatsapp: bool,
    pub detect_slack: bool,
    /// También se usa fuera de llamadas de trabajo: solo si se activa.
    pub detect_discord: bool,
    pub detect_browsers: bool,
    /// Liberar el modelo tras N min sin grabar (0 = nunca).
    pub release_after_min: u32,
    pub update_notified: String,
    pub last_run_version: String,
    // --- Anotalo ---
    pub hotkey_mark: String,
    /// Resultado de medir la GPU integrada contra el procesador (None = sin medir).
    pub igpu_faster: Option<bool>,
    /// La GPU integrada en la que se midió ("" = ninguna).
    pub igpu_name: String,
    /// Buscar actualizaciones al arrancar y cada 6 h.
    pub auto_update_check: bool,
    /// La instrucción de "Copiar para Claude" (en blanco = `DEFAULT_CLAUDE_INSTRUCTION`).
    pub claude_instruction: String,
    /// Dónde estaba la ventana al ocultarla: izquierda, arriba, derecha y abajo en píxeles físicos
    /// (None = centrada la próxima vez).
    pub window_rect: Option<[i32; 4]>,
    /// Claves desconocidas: se conservan.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Settings {
    pub fn defaults(p: Profile) -> Settings {
        Settings {
            model: "auto".into(),
            language: "es".into(),
            device: "auto".into(),
            compute_type: "float16".into(),
            label_me: "Yo".into(),
            label_others: "Otros".into(),
            hotkey: "<ctrl>+<alt>+r".into(),
            auto_mode: true,
            output_dir: default_output_dir(p).to_string_lossy().into_owned(),
            mic_device: None,
            silence_ms: 600,
            max_segment_s: 25.0,
            min_segment_ms: 300,
            save_audio: false,
            copy_on_stop: true,
            show_indicator: true,
            autostart: true,
            autostart_configured: false,
            detect_teams: true,
            detect_zoom: true,
            detect_whatsapp: true,
            detect_slack: true,
            detect_discord: false,
            detect_browsers: true,
            release_after_min: 10,
            update_notified: String::new(),
            last_run_version: String::new(),
            hotkey_mark: "<ctrl>+<alt>+m".into(),
            igpu_faster: None,
            igpu_name: String::new(),
            auto_update_check: true,
            claude_instruction: DEFAULT_CLAUDE_INSTRUCTION.into(),
            window_rect: None,
            extra: Map::new(),
        }
    }

    /// La instrucción de "Copiar para Claude": la del usuario, o la de por defecto si está en blanco.
    pub fn claude_instruction_or_default(&self) -> &str {
        if self.claude_instruction.trim().is_empty() {
            DEFAULT_CLAUDE_INSTRUCTION
        } else {
            &self.claude_instruction
        }
    }

    /// Cambia un ajuste por su nombre (desde la ventana). Error si no existe o si el valor no es
    /// del tipo del ajuste.
    pub fn set_field(&mut self, key: &str, value: Value) -> Result<(), String> {
        macro_rules! set {
            ($($field:ident),* $(,)?) => {
                match key {
                    $(stringify!($field) => {
                        self.$field = serde_json::from_value(value)
                            .map_err(|e| format!("Valor no válido para {key}: {e}"))?;
                        Ok(())
                    })*
                    _ => Err(format!("Ajuste desconocido: {key}")),
                }
            };
        }
        with_settings_fields!(set)
    }

    /// El interruptor de detección de una plataforma (`Platform::setting`).
    pub fn detect_enabled(&self, setting: &str) -> bool {
        match setting {
            "detect_teams" => self.detect_teams,
            "detect_zoom" => self.detect_zoom,
            "detect_whatsapp" => self.detect_whatsapp,
            "detect_slack" => self.detect_slack,
            "detect_discord" => self.detect_discord,
            "detect_browsers" => self.detect_browsers,
            _ => false,
        }
    }
}

pub fn config_path(p: Profile) -> PathBuf {
    data_dir(p).join("config.json")
}

/// Por qué no se pudo usar un `config.json` que existe (se cargan los valores por defecto).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadProblem {
    /// No es JSON (o no es un objeto). Se copió tal cual a `backup` (None si la copia falló).
    Damaged { backup: Option<PathBuf> },
    /// Existe pero no se pudo leer (bloqueado, sin permiso): no se debe guardar encima.
    Unreadable(String),
}

/// Intentos de lectura: un antivirus o OneDrive pueden tener el archivo bloqueado un momento.
const READ_TRIES: u32 = 3;

/// Nunca falla: sin archivo, o si no es un objeto JSON, valores por defecto. Cada clave se lee por
/// separado: una con un tipo que no toca se queda con su valor por defecto y las demás se conservan.
pub fn load_settings(path: &Path, p: Profile) -> Settings {
    load_settings_checked(path, p).0
}

/// Como `load_settings`, y además dice si había un archivo que no se pudo usar. Uno dañado se copia
/// a `config.json.bad` antes de que el siguiente guardado lo sobrescriba.
pub fn load_settings_checked(path: &Path, p: Profile) -> (Settings, Option<LoadProblem>) {
    let mut s = Settings::defaults(p);
    let mut tries = 0;
    let bytes = loop {
        tries += 1;
        match fs::read(path) {
            Ok(b) => break b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return (s, None),
            Err(e) if tries < READ_TRIES => {
                log::debug!("No se pudo leer {} ({e}); reintento", path.display());
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                log::warn!(
                    "No se pudo leer {} ({e}); uso valores por defecto",
                    path.display()
                );
                return (s, Some(LoadProblem::Unreadable(e.to_string())));
            }
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut map = match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(m)) => m,
        Ok(_) | Err(_) => {
            let bad = path.with_extension("json.bad");
            let backup = match fs::write(&bad, &bytes) {
                Ok(()) => Some(bad),
                Err(e) => {
                    log::error!("No se pudo copiar a {} ({e})", bad.display());
                    None
                }
            };
            log::warn!(
                "Config inválida en {}; uso valores por defecto (copia en {})",
                path.display(),
                backup
                    .as_ref()
                    .map_or("ninguna".into(), |b| b.display().to_string())
            );
            return (s, Some(LoadProblem::Damaged { backup }));
        }
    };
    // Versiones anteriores de la app de Python.
    if map.get("device").and_then(Value::as_str) == Some("cuda") {
        map.insert("device".into(), Value::from("auto"));
    }
    if let Some(meet) = map.remove("detect_meet")
        && !map.contains_key("detect_browsers")
    {
        map.insert("detect_browsers".into(), Value::Bool(truthy(&meet)));
    }
    macro_rules! take {
        ($($field:ident),* $(,)?) => {$(
            if let Some(v) = map.remove(stringify!($field)) {
                match serde_json::from_value(v) {
                    Ok(x) => s.$field = x,
                    Err(e) => log::warn!(
                        "Ajuste {} inválido en {} ({e}); uso el valor por defecto",
                        stringify!($field),
                        path.display()
                    ),
                }
            }
        )*};
    }
    with_settings_fields!(take);
    s.extra = map;
    (s, None)
}

/// Solo la carpeta de grabaciones de un `config.json`: su `output_dir` si no está vacío; si no (o si
/// falta el archivo o no es un objeto JSON), la de por defecto. Para quien solo lee (el servidor MCP,
/// el desinstalador): nunca escribe nada (ni la copia `.bad`) ni registra más que en debug.
pub fn read_output_dir(path: &Path, p: Profile) -> PathBuf {
    let dir = fs::read(path)
        .inspect_err(|e| log::debug!("No se pudo leer {} ({e})", path.display()))
        .ok()
        .and_then(|bytes| {
            let text = String::from_utf8_lossy(&bytes);
            serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
                .inspect_err(|e| log::debug!("Config inválida en {} ({e})", path.display()))
                .ok()
        })
        .and_then(|v| v.get("output_dir")?.as_str().map(str::to_string))
        .filter(|d| !d.trim().is_empty());
    dir.map_or_else(|| default_output_dir(p), PathBuf::from)
}

/// `bool(x)` de Python para lo que pudiera haber en `detect_meet`.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// JSON con sangría de 2 espacios y UTF-8 (el formato de la app de Python), escrito en un `.tmp`,
/// llevado al disco y renombrado: un corte (o un apagón) a mitad no deja el archivo a medias. Los
/// guardados van de uno en uno: dos a la vez (la medición de la GPU integrada y el menú de la
/// bandeja) se pisarían el `.tmp`.
pub fn save_settings(s: &Settings, path: &Path) -> io::Result<()> {
    static SAVING: Mutex<()> = Mutex::new(());
    let text = serde_json::to_string_pretty(s).map_err(io::Error::other)?;
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)
}

/// Primer arranque: copia los ajustes de la app de Python si Anotalo aún no tiene los suyos. La
/// carpeta antigua se conserva (hasta desinstalar).
pub fn migrate_legacy(p: Profile) -> io::Result<bool> {
    let appdata = data_dir(p)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    migrate_legacy_in(&appdata, p)
}

pub fn migrate_legacy_in(appdata: &Path, p: Profile) -> io::Result<bool> {
    let ours = data_dir_in(appdata, p).join("config.json");
    let legacy = legacy_data_dir_in(appdata, p).join("config.json");
    if ours.exists() || !legacy.exists() {
        return Ok(false);
    }
    fs::create_dir_all(data_dir_in(appdata, p))?;
    fs::copy(&legacy, &ours)?;
    log::info!("Ajustes copiados de {}", legacy.display());
    Ok(true)
}

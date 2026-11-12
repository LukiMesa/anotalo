//! Ajustes del usuario en `%APPDATA%\Anotalo\config.json`, con las mismas claves que la app de
//! Python (calltx/config.py). Las claves que ya no hacen nada (`compute_type`, `save_audio`) y las
//! desconocidas se leen y se vuelven a escribir tal cual; las nuevas de Anotalo van al final.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::paths::{Profile, data_dir, data_dir_in, default_output_dir, legacy_data_dir_in};

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
            extra: Map::new(),
        }
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

/// Nunca falla: sin archivo, o si no es un objeto JSON, valores por defecto. Cada clave se lee por
/// separado: una con un tipo que no toca se queda con su valor por defecto y las demás se conservan.
pub fn load_settings(path: &Path, p: Profile) -> Settings {
    let mut s = Settings::defaults(p);
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return s,
        Err(e) => {
            log::warn!(
                "No se pudo leer {} ({e}); uso valores por defecto",
                path.display()
            );
            return s;
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut map = match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(m)) => m,
        Ok(_) | Err(_) => {
            log::warn!(
                "Config inválida en {}; uso valores por defecto",
                path.display()
            );
            return s;
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
    take!(
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
    );
    s.extra = map;
    s
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

/// JSON con sangría de 2 espacios y UTF-8 (el formato de la app de Python), escrito en un `.tmp` y
/// renombrado: un corte a mitad no deja el archivo a medias.
pub fn save_settings(s: &Settings, path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(s).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
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

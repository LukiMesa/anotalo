//! Poner nombre a las personas de una llamada ya grabada (spec §2.4.5). Cada llamada guarda las
//! huellas de sus voces en `%APPDATA%\Anotalo\llamadas\<archivo .md>.json`: así, al nombrar a
//! "Persona 1", su huella pasa a la memoria de voces.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::diarizer::CallVoice;

/// Las voces de una llamada.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct CallPrints {
    pub version: u32,
    /// Nombre del archivo `.md` de la llamada.
    pub file: String,
    pub voices: Vec<CallVoice>,
}

/// `<prints_dir>\<archivo .md>.json`.
pub fn call_prints_path(prints_dir: &Path, transcript: &Path) -> PathBuf {
    let name = transcript
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    prints_dir.join(format!("{name}.json"))
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

/// Guarda las voces de la llamada de `transcript`.
pub fn save_call_prints(
    prints_dir: &Path,
    transcript: &Path,
    voices: Vec<CallVoice>,
) -> io::Result<()> {
    let prints = CallPrints {
        version: 1,
        file: transcript
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        voices,
    };
    write_json(&call_prints_path(prints_dir, transcript), &prints)
}

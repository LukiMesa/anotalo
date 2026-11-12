//! Poner nombre a las personas de una llamada ya grabada (spec §2.4.5). Cada llamada guarda las
//! huellas de sus voces en `%APPDATA%\Anotalo\llamadas\<archivo .md>.json`: así, al nombrar a
//! "Persona 1", su huella pasa a la memoria de voces.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::diarizer::{CallVoice, PERSON};
use super::memory::{VoiceStore, mix};

/// Las huellas de cada llamada se guardan 30 días: lo justo para nombrar a alguien después.
pub const KEEP_CALL_PRINTS: Duration = Duration::from_secs(30 * 86_400);
const MAX_NAME_CHARS: usize = 40;

/// Un nombre que el Markdown (y el lector de la app de Python) aguanta: sin `* : [ ]`, sin
/// caracteres de control ni separadores de línea, 1–40 caracteres, y que no sea "Marca".
pub fn validate_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Nombre no válido: está vacío".into());
    }
    if n.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "Nombre no válido: más de {MAX_NAME_CHARS} caracteres"
        ));
    }
    if n.chars()
        .any(|c| matches!(c, '*' | ':' | '[' | ']' | '\u{2028}' | '\u{2029}') || c.is_control())
    {
        return Err("Nombre no válido: no puede llevar * : [ ] ni saltos de línea".into());
    }
    if n.eq_ignore_ascii_case("marca") {
        return Err("Nombre no válido: «Marca» es el nombre de las marcas".into());
    }
    Ok(n.to_string())
}

/// La línea con el hablante cambiado, si es una línea de `from` (`**[HH:MM:SS] from:** …`).
fn renamed_line(line: &str, from: &str, to: &str) -> Option<String> {
    let ts = line.strip_prefix("**[")?.get(..8)?;
    let ok = ts.bytes().enumerate().all(|(i, b)| {
        if i == 2 || i == 5 {
            b == b':'
        } else {
            b.is_ascii_digit()
        }
    });
    let rest = line.get(11..)?.strip_prefix(&format!("] {from}:** "))?;
    ok.then(|| format!("**[{ts}] {to}:** {rest}"))
}

/// La app tiene el archivo abierto mientras graba la llamada (y lo reescribe al colgar).
#[cfg(windows)]
fn being_recorded(md: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    matches!(
        fs::OpenOptions::new().read(true).share_mode(0).open(md),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION)
    )
}

#[cfg(not(windows))]
fn being_recorded(_md: &Path) -> bool {
    false
}

/// Cambia `from` por `to` en las líneas de esa persona (y solo en ellas). Devuelve cuántas.
pub fn rename_speaker(md: &Path, from: &str, to: &str) -> Result<usize, String> {
    let to = validate_name(to)?;
    if being_recorded(md) {
        return Err("La llamada aún se está grabando; ponle nombre al colgar".into());
    }
    let text =
        fs::read_to_string(md).map_err(|e| format!("No se pudo leer {}: {e}", md.display()))?;
    let mut n = 0;
    let out: String = text
        .split_inclusive('\n')
        .map(|l| match renamed_line(l, from, &to) {
            Some(r) => {
                n += 1;
                r
            }
            None => l.to_string(),
        })
        .collect();
    let file = md
        .file_name()
        .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    if n == 0 {
        return Err(format!("No hay líneas de «{from}» en {file}"));
    }
    let tmp = md.with_extension("md.tmp");
    fs::write(&tmp, out)
        .and_then(|()| fs::rename(&tmp, md))
        .map_err(|e| format!("No se pudo guardar {file}: {e}"))?;
    Ok(n)
}

pub struct NameOutcome {
    pub lines: usize,
    /// Había huella de esa persona en la llamada y ahora está en la memoria de voces.
    pub remembered: bool,
}

/// Una etiqueta de la app ("Persona 3", `label_me`, `label_others`), no el nombre de alguien.
fn is_reserved(name: &str, reserved: &[&str]) -> bool {
    let n = name.trim();
    reserved
        .iter()
        .any(|r| r.to_lowercase() == n.to_lowercase())
        || n.strip_prefix(PERSON)
            .and_then(|r| r.strip_prefix(' '))
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// Nombra a `from` como `to` en la llamada `md` y, si la llamada guardó su huella, la recuerda.
/// Si `to` es una etiqueta de la app (`reserved`, "Persona N") solo se corrige el archivo: es
/// juntar a una persona partida en dos, no un nombre. Si `from` ya era un nombre puesto por el
/// usuario en esta llamada, es una corrección y se cambia el nombre de la voz guardada.
pub fn name_in_call(
    md: &Path,
    prints_dir: &Path,
    store: &mut VoiceStore,
    from: &str,
    to: &str,
    reserved: &[&str],
) -> Result<NameOutcome, String> {
    let to = validate_name(to)?;
    let lines = rename_speaker(md, from, &to)?;
    let path = call_prints_path(prints_dir, md);
    let mut prints: Option<CallPrints> = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let Some(p) = prints.as_mut() else {
        return Ok(NameOutcome {
            lines,
            remembered: false,
        });
    };
    let Some(i) = p.voices.iter().position(|v| v.label == from) else {
        return Ok(NameOutcome {
            lines,
            remembered: false,
        });
    };
    let remembered = !is_reserved(&to, reserved);
    if remembered {
        let v = &p.voices[i];
        if v.named && store.voices.iter().any(|s| s.name == from) {
            store.rename(from, &to)?;
        } else {
            store.remember(&to, &v.print, v.seconds);
        }
        store
            .save()
            .map_err(|e| format!("No se pudo guardar la voz: {e}"))?;
    }
    // la etiqueta en las voces de la llamada (si ya había otra igual, es la misma persona)
    match p.voices.iter().position(|v| v.label == to) {
        Some(j) if j != i => {
            let v = p.voices.remove(i);
            let j = if j > i { j - 1 } else { j };
            let w = &mut p.voices[j];
            w.print = mix(&w.print, w.seconds, &v.print, v.seconds);
            w.seconds += v.seconds;
            w.named |= remembered;
        }
        _ => {
            p.voices[i].label = to;
            p.voices[i].named = remembered;
        }
    }
    if let Err(e) = write_json(&path, p) {
        log::warn!("No se pudieron actualizar las voces de la llamada: {e}");
    }
    Ok(NameOutcome { lines, remembered })
}

/// Borra las huellas de llamadas más antiguas que `older_than`. Devuelve cuántas.
pub fn prune_call_prints(prints_dir: &Path, older_than: Duration) -> usize {
    let Ok(entries) = fs::read_dir(prints_dir) else {
        return 0;
    };
    let now = SystemTime::now();
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > older_than)
        })
        .filter(|e| fs::remove_file(e.path()).is_ok())
        .count()
}

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

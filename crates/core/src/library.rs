//! Las grabaciones son los Markdown de la carpeta de salida: aquí se leen, buscan y borran (port
//! de calltx/library.py). Además se leen las líneas que Anotalo añade y la app de Python ignora:
//! `- Título:`, `- Cliente:` y las marcas (`**[HH:MM:SS] Marca**`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{NaiveDate, NaiveDateTime, TimeDelta};
use regex::{Captures, Regex};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::canonical_combining_class;

/// Etiquetas de las notas rápidas ("Manual" era la de las versiones antiguas).
pub const NOTE_APPS: [&str; 2] = ["Nota", "Manual"];
pub const TRASH_DIR: &str = ".papelera";
const SNIPPET_CHARS: usize = 160;

// Las expresiones de la app de Python (`_NAME`, `_TITLE`, `_FIELD`, `_LINE`), con dígitos ASCII.
static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{4})-([0-9]{2})-([0-9]{2})_([0-9]{2})([0-9]{2})_([^_.]+)(?:_[0-9]+)?\.md$")
        .expect("expresión válida")
});
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^# Llamada — (.+?) — ").expect("expresión válida"));
static FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^- (Inicio|Fin|Duración): ([0-9]{2}):([0-9]{2}):([0-9]{2})\s*$")
        .expect("expresión válida")
});
static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\*\*\[([0-9]{2}):([0-9]{2}):([0-9]{2})\] (.+?):\*\* (.*)$")
        .expect("expresión válida")
});
static MARK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\*\*\[([0-9]{2}):([0-9]{2}):([0-9]{2})\] Marca\*\*\s*$")
        .expect("expresión válida")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub ts: String,
    pub start_s: u32,
    /// Vacío en una marca.
    pub speaker: String,
    /// Vacío en una marca.
    pub text: String,
    pub mark: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Call,
    Note,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Call => "call",
            Kind::Note => "note",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Recording {
    /// El nombre del archivo.
    pub id: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub app: String,
    /// None si el nombre no trae una fecha posible.
    pub started_at: Option<NaiveDateTime>,
    pub ended_at: Option<NaiveDateTime>,
    pub duration_s: Option<u32>,
    /// El título de la reunión (`- Título:`).
    pub title: Option<String>,
    /// `- Cliente:`.
    pub client: Option<String>,
    pub lines: Vec<Line>,
}

impl Recording {
    /// Las líneas dichas (sin las marcas).
    fn said(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter().filter(|l| !l.mark)
    }

    /// El principio del texto para la lista: lo que dijeron los demás (o, si no hay, todo).
    pub fn snippet(&self, label_me: &str) -> String {
        let others: Vec<&str> = self
            .said()
            .filter(|l| l.speaker != label_me)
            .map(|l| l.text.as_str())
            .collect();
        let texts = if others.is_empty() {
            self.said().map(|l| l.text.as_str()).collect()
        } else {
            others
        };
        texts.join(" ").chars().take(SNIPPET_CHARS).collect()
    }

    /// Las personas de la llamada (sin `label_me`), en el orden en que hablan por primera vez.
    pub fn people(&self, label_me: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for l in self.said() {
            if l.speaker != label_me && !out.contains(&l.speaker) {
                out.push(l.speaker.clone());
            }
        }
        out
    }
}

/// Para buscar sin distinguir mayúsculas ni tildes: minúsculas, NFKD y sin marcas combinantes.
pub fn fold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.to_lowercase()
        .nfkd()
        .filter(|c| canonical_combining_class(*c) == 0)
        .collect()
}

/// Los saltos de línea de `str.splitlines()` de Python.
fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\x0b'
            | '\x0c'
            | '\x1c'
            | '\x1d'
            | '\x1e'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `str.splitlines()` de Python (`\r\n` cuenta como un salto).
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let Some(i) = rest.find(is_line_break) else {
            return Some(std::mem::take(&mut rest));
        };
        let line = &rest[..i];
        let after = &rest[i..];
        let skip = if after.starts_with("\r\n") {
            2
        } else {
            after.chars().next().map_or(1, char::len_utf8)
        };
        rest = &after[skip..];
        Some(line)
    })
}

/// `str.strip()` de Python.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

fn num(c: &Captures, i: usize) -> u32 {
    c[i].parse().unwrap_or(0)
}

/// (día, inicio) del nombre del archivo; None si la fecha o la hora son imposibles.
fn name_start(m: &Captures) -> Option<(NaiveDate, NaiveDateTime)> {
    let year = i32::try_from(num(m, 1)).ok().filter(|y| *y >= 1)?;
    let day = NaiveDate::from_ymd_opt(year, num(m, 2), num(m, 3))?;
    Some((day, day.and_hms_opt(num(m, 4), num(m, 5), 0)?))
}

/// El valor de una línea de cabecera de Anotalo (`- Título: …`), si lo tiene.
fn header_value(raw: &str, key: &str) -> Option<String> {
    let v = py_strip(raw.strip_prefix(key)?);
    (!v.is_empty()).then(|| v.to_string())
}

/// Lee una grabación. Nunca falla: lo que no se entiende se ignora y un archivo que no se puede
/// leer se queda sin líneas.
pub fn parse_recording(path: &Path) -> Recording {
    let id = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let m = NAME.captures(&id);
    let mut app = m.as_ref().map_or_else(String::new, |m| m[6].to_string());
    let (day, mut started_at) = match m.as_ref().and_then(name_start) {
        Some((d, s)) => (Some(d), Some(s)),
        None => (None, None),
    };
    let mut ended_at = None;
    let mut duration_s = None;
    let (mut title, mut client) = (None, None);
    let mut in_header = true;
    let mut lines = Vec::new();
    let text = fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    for raw in split_lines(&text) {
        if app.is_empty()
            && let Some(t) = TITLE.captures(raw)
        {
            app = t[1].to_string();
        } else if let Some(f) = FIELD.captures(raw) {
            let (h, mi, s) = (num(&f, 2), num(&f, 3), num(&f, 4));
            match (&f[1], day) {
                ("Inicio" | "Fin", Some(_)) if !(h < 24 && mi < 60 && s < 60) => {} // hora imposible
                ("Inicio", Some(d)) => started_at = d.and_hms_opt(h, mi, s),
                ("Fin", Some(d)) => ended_at = d.and_hms_opt(h, mi, s),
                ("Duración", _) => duration_s = Some(h * 3600 + mi * 60 + s),
                _ => {}
            }
        } else if let Some(l) = LINE.captures(raw) {
            lines.push(Line {
                ts: format!("{}:{}:{}", &l[1], &l[2], &l[3]),
                start_s: num(&l, 1) * 3600 + num(&l, 2) * 60 + num(&l, 3),
                speaker: l[4].to_string(),
                text: py_strip(&l[5]).to_string(),
                mark: false,
            });
        } else if let Some(k) = MARK.captures(raw) {
            lines.push(Line {
                ts: format!("{}:{}:{}", &k[1], &k[2], &k[3]),
                start_s: num(&k, 1) * 3600 + num(&k, 2) * 60 + num(&k, 3),
                speaker: String::new(),
                text: String::new(),
                mark: true,
            });
        } else if in_header {
            if raw.trim_end() == "---" {
                in_header = false;
            } else if title.is_none() && raw.starts_with("- Título:") {
                title = header_value(raw, "- Título:");
            } else if client.is_none() && raw.starts_with("- Cliente:") {
                client = header_value(raw, "- Cliente:");
            }
        }
    }
    if let (Some(end), Some(start)) = (ended_at, started_at)
        && end < start
    {
        ended_at = Some(end + TimeDelta::days(1));
    }
    let kind = if NOTE_APPS.contains(&app.as_str()) {
        Kind::Note
    } else {
        Kind::Call
    };
    Recording {
        id,
        path: path.to_path_buf(),
        kind,
        app,
        started_at,
        ended_at,
        duration_s,
        title,
        client,
        lines,
    }
}

fn is_md(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_lowercase().ends_with(".md"))
}

fn matches_query(r: &Recording, q: &str) -> bool {
    r.lines.iter().any(|l| fold(&l.text).contains(q))
        || [&r.title, &r.client]
            .into_iter()
            .flatten()
            .any(|v| fold(v).contains(q))
}

/// Las grabaciones de la carpeta (sin entrar en `.papelera`), de la más reciente a la más antigua.
/// `kind` = call | note | cualquier otra cosa (todas). La búsqueda no distingue mayúsculas ni
/// tildes y mira el texto, el título y el cliente; `client` filtra por ese cliente exacto, sin
/// distinguir mayúsculas ni tildes (vacío = todos).
pub fn list_recordings(
    folder: &Path,
    kind: &str,
    query: &str,
    client: Option<&str>,
) -> Vec<Recording> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut recs: Vec<Recording> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_md(p) && p.is_file())
        .map(|p| parse_recording(&p))
        .collect();
    if let k @ ("call" | "note") = kind {
        recs.retain(|r| r.kind.as_str() == k);
    }
    let q = fold(py_strip(query));
    if !q.is_empty() {
        recs.retain(|r| matches_query(r, &q));
    }
    let c = fold(py_strip(client.unwrap_or("")));
    if !c.is_empty() {
        recs.retain(|r| r.client.as_deref().is_some_and(|rc| fold(rc) == c));
    }
    recs.sort_by(|a, b| (b.started_at, &b.id).cmp(&(a.started_at, &a.id)));
    recs
}

/// Solo el texto dicho, en párrafos.
pub fn to_plain_text(rec: &Recording) -> String {
    rec.lines
        .iter()
        .filter(|l| !l.text.is_empty())
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `[HH:MM:SS] Quién: texto`, una línea por frase (las marcas, `[HH:MM:SS] Marca`).
pub fn to_readable(rec: &Recording) -> String {
    rec.lines
        .iter()
        .map(|l| {
            if l.mark {
                format!("[{}] Marca", l.ts)
            } else {
                format!("[{}] {}: {}", l.ts, l.speaker, l.text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn now_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn invalid_token() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "Token de papelera no válido")
}

/// Borrar = mover a una carpeta oculta; deshacer = devolverlo.
pub struct Trash {
    folder: PathBuf,
    dir: PathBuf,
}

impl Trash {
    pub fn new(folder: &Path) -> Trash {
        Trash {
            folder: folder.to_path_buf(),
            dir: folder.join(TRASH_DIR),
        }
    }

    /// Mueve el archivo a la papelera. Devuelve el token para deshacerlo (`<ms>__<nombre>`).
    pub fn delete(&self, path: &Path) -> io::Result<String> {
        fs::create_dir_all(&self.dir)?;
        hide(&self.dir);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Ruta sin nombre"))?;
        let token = format!("{}__{name}", (now_s() * 1000.0) as u64);
        fs::rename(path, self.dir.join(&token))?;
        Ok(token)
    }

    /// Devuelve el archivo a su carpeta (con `_2`, `_3`… si ya hay otro con su nombre).
    pub fn undo(&self, token: &str) -> io::Result<PathBuf> {
        let (ts, name) = token.split_once("__").ok_or_else(invalid_token)?;
        let plain_name = Path::new(name).file_name().and_then(|n| n.to_str()) == Some(name)
            && !name.contains([':', '/', '\\']);
        if ts.is_empty()
            || !ts.bytes().all(|b| b.is_ascii_digit())
            || !plain_name
            || !name.ends_with(".md")
        {
            return Err(invalid_token());
        }
        let stem = Path::new(name)
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let mut dest = self.folder.join(name);
        let mut n = 2;
        while dest.exists() {
            dest = self.folder.join(format!("{stem}_{n}.md"));
            n += 1;
        }
        fs::rename(self.dir.join(token), &dest)?;
        Ok(dest)
    }

    /// Borra de verdad lo que lleva en la papelera más de `older_than_s` segundos. Devuelve
    /// cuántos.
    pub fn purge(&self, older_than_s: f64) -> io::Result<usize> {
        if !self.dir.is_dir() {
            return Ok(0);
        }
        let limit = (now_s() - older_than_s) * 1000.0;
        let mut n = 0;
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let ts = name.split("__").next().unwrap_or_default();
            let old = !ts.is_empty()
                && ts.bytes().all(|b| b.is_ascii_digit())
                && ts.parse::<f64>().is_ok_and(|t| t < limit);
            if old {
                match fs::remove_file(entry.path()) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                n += 1;
            }
        }
        Ok(n)
    }
}

/// Carpeta oculta en el Explorador (como la papelera de la app de Python).
#[cfg(windows)]
fn hide(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetFileAttributesW(name: *const u16, attributes: u32) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: cadena UTF-16 terminada en cero, válida durante la llamada.
    if unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN) } == 0 {
        log::debug!("No se pudo ocultar {}", dir.display());
    }
}

#[cfg(not(windows))]
fn hide(_dir: &Path) {}

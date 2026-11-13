//! Las cuatro herramientas de `Anotalo.exe mcp` sobre la carpeta de grabaciones: listar, buscar,
//! leer una llamada (por páginas si es larga) y la más reciente. Solo leen: aquí no hay nada que
//! escriba, mueva o borre un archivo (un test lo comprueba en el código fuente).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{Map, Value, json};

use crate::config::{config_path, read_output_dir};
use crate::library::{Kind, Recording, fold, list_recordings, parse_recording};
use crate::mcp::server::{ToolSpec, Tools, serve};
use crate::paths::Profile;

/// Caracteres de transcripción por respuesta de `get_call`, por debajo del límite de 25 000 tokens
/// que Claude Code admite en lo que devuelve una herramienta.
pub const BUDGET_CHARS: usize = 50_000;
/// Una frase más larga se recorta a esto, alrededor de lo encontrado.
pub const SNIPPET_CHARS: usize = 240;
/// Fragmentos por llamada en `search_calls`.
pub const MAX_SNIPPETS: usize = 5;
pub const LIST_LIMIT: usize = 20;
pub const SEARCH_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 100;

const LIST_CALLS: &str = "Lista las llamadas grabadas por Anotalo, de la más reciente a la más antigua, con título, cliente, plataforma, fecha, duración, personas y número de marcas. Filtra por cliente, plataforma, fechas o personas.";
const SEARCH_CALLS: &str = "Busca un texto en lo que se dijo en las llamadas (sin distinguir mayúsculas ni tildes) y devuelve fragmentos con la hora y quién lo dijo.";
const GET_CALL: &str = "Devuelve una llamada completa: sus datos (título, cliente, plataforma, personas, marcas) y la transcripción en Markdown. Las líneas «Marca» señalan momentos que el usuario marcó durante la llamada, a menudo acuerdos.";
const GET_LATEST_CALL: &str =
    "Devuelve la llamada más reciente, o la más reciente con un cliente, igual que get_call.";

const CLIENT: &str = "El cliente, o parte de su nombre (sin distinguir mayúsculas ni tildes).";
const PLATFORM: &str = "Teams, Zoom, Meet, WhatsApp, Slack, Discord, Llamada (manual)…";
const FROM: &str = "Desde este día, incluido (AAAA-MM-DD).";
const TO: &str = "Hasta este día, incluido (AAAA-MM-DD).";
const NOTES: &str = "Incluir también las notas rápidas.";
const FROM_LINE: &str = "Primera línea de la transcripción que devolver (1 = el principio). Una llamada larga llega por partes: para seguir, usa el next_from_line de la respuesta anterior.";

type Args = Map<String, Value>;

/// Las herramientas sobre una carpeta de grabaciones.
pub struct LibraryTools {
    folder: PathBuf,
}

/// `Anotalo.exe mcp`: sirve las herramientas por stdin/stdout sobre la carpeta de los ajustes
/// (leída sin tocar `config.json`) hasta que Claude Code cierra la entrada.
pub fn run_stdio(p: Profile) -> io::Result<()> {
    let folder = read_output_dir(&config_path(p), p);
    log::info!("Servidor MCP sobre {}", folder.display());
    let tools = LibraryTools::new(folder);
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(stdin.lock(), &mut stdout.lock(), &tools)
}

impl LibraryTools {
    pub fn new(folder: PathBuf) -> LibraryTools {
        LibraryTools { folder }
    }

    /// Las grabaciones que pasan el filtro, de la más reciente a la más antigua.
    fn recordings(&self, f: &Filter) -> Vec<Recording> {
        list_recordings(&self.folder, "all", "", None)
            .into_iter()
            .filter(|r| f.keeps(r))
            .collect()
    }

    /// El archivo de un id: un nombre `.md` (se añade si falta) que está directamente en la
    /// carpeta. Nada de rutas, ni la papelera, ni otra carpeta. `.MD` también vale: la lista los
    /// incluye.
    fn resolve(&self, id: &str) -> Option<PathBuf> {
        let name = if id.to_lowercase().ends_with(".md") {
            id.to_string()
        } else {
            format!("{id}.md")
        };
        let plain = Path::new(&name).file_name().and_then(|n| n.to_str()) == Some(name.as_str())
            && !name.contains([':', '/', '\\']);
        let path = self.folder.join(&name);
        (plain && path.is_file()).then_some(path)
    }

    fn list_calls(&self, args: &Args) -> Result<Vec<String>, String> {
        let f = Filter::parse(args, true)?;
        let limit = limit(args, LIST_LIMIT)?;
        let recs = self.recordings(&f);
        let calls: Vec<Value> = recs.iter().take(limit).map(summary).collect();
        let out = json!({
            "folder": self.folder.display().to_string(),
            "total": recs.len(),
            "calls": calls,
        });
        Ok(vec![out.to_string()])
    }

    fn search_calls(&self, args: &Args) -> Result<Vec<String>, String> {
        const MISSING: &str = "Falta el texto que buscar (query).";
        let query = text(args, "query")?.ok_or(MISSING)?;
        let q = fold(query);
        if q.trim().is_empty() {
            return Err(MISSING.into());
        }
        let f = Filter::parse(args, false)?;
        let limit = limit(args, SEARCH_LIMIT)?;
        let mut total = 0;
        let mut calls = Vec::new();
        for r in self.recordings(&f) {
            let hits: Vec<_> = r
                .lines
                .iter()
                .filter(|l| !l.mark && fold(&l.text).contains(&q))
                .collect();
            if hits.is_empty() {
                continue;
            }
            total += 1;
            if calls.len() == limit {
                continue;
            }
            let snippets: Vec<Value> = hits
                .iter()
                .take(MAX_SNIPPETS)
                .map(|l| json!({"ts": l.ts, "speaker": l.speaker, "text": cut(&l.text, &q)}))
                .collect();
            calls.push(json!({
                "id": r.id,
                "platform": r.app,
                "title": r.title,
                "client": r.client,
                "started_at": stamp(r.started_at),
                "matches": hits.len(),
                "snippets": snippets,
            }));
        }
        Ok(vec![
            json!({"query": query, "total": total, "calls": calls}).to_string(),
        ])
    }

    fn get_call(&self, args: &Args) -> Result<Vec<String>, String> {
        let id = match args.get("id") {
            None | Some(Value::Null) => {
                return Err(
                    "Falta el id de la llamada (el nombre del archivo, de list_calls).".into(),
                );
            }
            Some(Value::String(s)) => s.trim().to_string(),
            Some(other) => other.to_string(),
        };
        let path = self
            .resolve(&id)
            .ok_or_else(|| format!("No existe la llamada «{id}»."))?;
        read_call(&path, from_line(args)?)
    }

    fn get_latest_call(&self, args: &Args) -> Result<Vec<String>, String> {
        let client = text(args, "client")?;
        let from = from_line(args)?;
        // Sin `include_notes`: solo llamadas, también la que está en curso.
        let f = Filter {
            client: client.map(fold),
            ..Filter::default()
        };
        match self.recordings(&f).first() {
            Some(r) => read_call(&r.path, from),
            None => Ok(vec![match client {
                Some(c) => format!("No hay llamadas con «{c}»."),
                None => "No hay llamadas.".into(),
            }]),
        }
    }
}

impl Tools for LibraryTools {
    fn specs(&self) -> Vec<ToolSpec> {
        let limit_prop = |default: usize| {
            json!({"type": "integer", "default": default, "minimum": 1, "maximum": MAX_LIMIT,
                   "description": "Cuántas devolver como máximo."})
        };
        vec![
            ToolSpec {
                name: "list_calls",
                description: LIST_CALLS,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "client": {"type": "string", "description": CLIENT},
                        "platform": {"type": "string", "description": PLATFORM},
                        "people": {"type": "array", "items": {"type": "string"},
                                   "description": "Personas que hablaron: todas deben haber hablado."},
                        "from": {"type": "string", "description": FROM},
                        "to": {"type": "string", "description": TO},
                        "include_notes": {"type": "boolean", "default": false, "description": NOTES},
                        "limit": limit_prop(LIST_LIMIT),
                    },
                }),
            },
            ToolSpec {
                name: "search_calls",
                description: SEARCH_CALLS,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "El texto que buscar."},
                        "client": {"type": "string", "description": CLIENT},
                        "platform": {"type": "string", "description": PLATFORM},
                        "from": {"type": "string", "description": FROM},
                        "to": {"type": "string", "description": TO},
                        "include_notes": {"type": "boolean", "default": false, "description": NOTES},
                        "limit": limit_prop(SEARCH_LIMIT),
                    },
                    "required": ["query"],
                }),
            },
            ToolSpec {
                name: "get_call",
                description: GET_CALL,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string",
                               "description": "El id de la llamada (el nombre del archivo, como lo dan list_calls y search_calls)."},
                        "from_line": {"type": "integer", "minimum": 1, "default": 1, "description": FROM_LINE},
                    },
                    "required": ["id"],
                }),
            },
            ToolSpec {
                name: "get_latest_call",
                description: GET_LATEST_CALL,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "client": {"type": "string", "description": CLIENT},
                        "from_line": {"type": "integer", "minimum": 1, "default": 1, "description": FROM_LINE},
                    },
                }),
            },
        ]
    }

    fn call(&self, name: &str, args: &Args) -> Result<Vec<String>, String> {
        match name {
            "list_calls" => self.list_calls(args),
            "search_calls" => self.search_calls(args),
            "get_call" => self.get_call(args),
            "get_latest_call" => self.get_latest_call(args),
            other => Err(format!("Herramienta desconocida: {other}")),
        }
    }
}

/// Los filtros de `list_calls`, `search_calls` y `get_latest_call`, ya plegados.
#[derive(Default)]
struct Filter {
    client: Option<String>,
    platform: Option<String>,
    people: Vec<String>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    notes: bool,
}

impl Filter {
    /// `people` solo en `list_calls`.
    fn parse(args: &Args, with_people: bool) -> Result<Filter, String> {
        Ok(Filter {
            client: text(args, "client")?.map(fold),
            platform: text(args, "platform")?.map(fold),
            people: if with_people {
                people(args)?
            } else {
                Vec::new()
            },
            from: date(args, "from")?,
            to: date(args, "to")?,
            notes: flag(args, "include_notes")?,
        })
    }

    fn keeps(&self, r: &Recording) -> bool {
        if r.kind == Kind::Note && !self.notes {
            return false;
        }
        if let Some(c) = &self.client
            && !r
                .client
                .as_deref()
                .is_some_and(|rc| fold(rc).contains(c.as_str()))
        {
            return false;
        }
        if let Some(p) = &self.platform
            && fold(&r.app) != *p
        {
            return false;
        }
        let day = r.started_at.map(|d| d.date());
        if self.from.is_some_and(|f| day.is_none_or(|d| d < f))
            || self.to.is_some_and(|t| day.is_none_or(|d| d > t))
        {
            return false;
        }
        if !self.people.is_empty() {
            let spoke: Vec<String> = speakers(r).iter().map(|s| fold(s)).collect();
            return self
                .people
                .iter()
                .all(|p| spoke.iter().any(|s| s.contains(p.as_str())));
        }
        true
    }
}

/// Un texto; vacío cuenta como si no estuviera.
fn text<'a>(args: &'a Args, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim()).filter(|s| !s.is_empty())),
        Some(_) => Err(format!("«{key}» debe ser un texto.")),
    }
}

/// Un entero (también `3.0` o `"3"`).
fn int(args: &Args, key: &str) -> Result<Option<i64>, String> {
    let bad = || format!("«{key}» debe ser un número entero.");
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| {
                n.as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f.trunc() as i64)
            })
            .map(Some)
            .ok_or_else(bad),
        Some(Value::String(s)) => s.trim().parse().map(Some).map_err(|_| bad()),
        Some(_) => Err(bad()),
    }
}

fn flag(args: &Args, key: &str) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("«{key}» debe ser true o false.")),
    }
}

fn date(args: &Args, key: &str) -> Result<Option<NaiveDate>, String> {
    let s = match args.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(other) => other.to_string(),
    };
    if s.is_empty() {
        return Ok(None);
    }
    NaiveDate::parse_from_str(&s, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| format!("Fecha no válida: {s} (usa AAAA-MM-DD)"))
}

/// Las personas pedidas, plegadas (un solo nombre también vale).
fn people(args: &Args) -> Result<Vec<String>, String> {
    let names: Vec<&Value> = match args.get("people") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v @ Value::String(_)) => vec![v],
        Some(_) => return Err("«people» debe ser una lista de nombres.".into()),
    };
    let mut out = Vec::new();
    for n in names {
        let Value::String(s) = n else {
            return Err("«people» debe ser una lista de nombres.".into());
        };
        let f = fold(s.trim());
        if !f.is_empty() {
            out.push(f);
        }
    }
    Ok(out)
}

/// `limit`, entre 1 y `MAX_LIMIT`.
fn limit(args: &Args, default: usize) -> Result<usize, String> {
    Ok(int(args, "limit")?.map_or(default, |n| n.clamp(1, MAX_LIMIT as i64) as usize))
}

/// `from_line`, desde 1.
fn from_line(args: &Args) -> Result<usize, String> {
    Ok(int(args, "from_line")?.map_or(1, |n| n.max(1) as usize))
}

/// Todas las personas que hablan, también el usuario («Yo»), en el orden en que hablan por primera
/// vez. (`Recording::people` quita a quien se le diga; ninguna frase tiene el hablante vacío.)
fn speakers(r: &Recording) -> Vec<String> {
    r.people("")
}

/// `AAAA-MM-DDTHH:MM:SS`, o null.
fn stamp(d: Option<NaiveDateTime>) -> Value {
    d.map_or(Value::Null, |d| {
        Value::from(d.format("%Y-%m-%dT%H:%M:%S").to_string())
    })
}

fn summary(r: &Recording) -> Value {
    json!({
        "id": r.id,
        "kind": r.kind.as_str(),
        "platform": r.app,
        "title": r.title,
        "client": r.client,
        "started_at": stamp(r.started_at),
        "duration_s": r.duration_s,
        "people": speakers(r),
        "marks": r.lines.iter().filter(|l| l.mark).count(),
        "finished": r.ended_at.is_some(),
    })
}

/// Los dos bloques de `get_call`: los datos de la llamada (JSON) y su Markdown desde `from`.
fn read_call(path: &Path, from: usize) -> Result<Vec<String>, String> {
    let rec = parse_recording(path);
    let bytes = fs::read(path).map_err(|e| {
        log::debug!("No se pudo leer {} ({e})", path.display());
        format!("No se pudo leer la llamada «{}» ({e}).", rec.id)
    })?;
    let text = String::from_utf8_lossy(&bytes);
    let page = page(&text, from)?;
    let meta = json!({
        "id": rec.id,
        "kind": rec.kind.as_str(),
        "platform": rec.app,
        "title": rec.title,
        "client": rec.client,
        "started_at": stamp(rec.started_at),
        "ended_at": stamp(rec.ended_at),
        "duration_s": rec.duration_s,
        "people": speakers(&rec),
        "marks": rec.lines.iter().filter(|l| l.mark).map(|l| l.ts.as_str()).collect::<Vec<_>>(),
        "finished": rec.ended_at.is_some(),
        "lines_total": page.total,
        "from_line": from,
        "next_from_line": page.next,
    });
    Ok(vec![meta.to_string(), page.text])
}

struct Page {
    text: String,
    /// Entradas del cuerpo.
    total: usize,
    /// La primera entrada que no cabe, o None si se llegó al final.
    next: Option<usize>,
}

/// El Markdown desde la entrada `from` (1 = la primera). Las entradas son las líneas no vacías tras
/// el primer `---` (frases y marcas). Si todo cabe en `BUDGET_CHARS` desde el principio, el archivo
/// tal cual; si no, la cabecera (hasta `---`), una línea en blanco y las entradas que quepan
/// separadas por una línea en blanco (al menos una).
fn page(text: &str, from: usize) -> Result<Page, String> {
    let lines: Vec<&str> = text.lines().collect();
    let (header, body) = match lines.iter().position(|l| l.trim_end() == "---") {
        Some(i) => lines.split_at(i + 1),
        None => (&lines[..0], &lines[..]),
    };
    let entries: Vec<&str> = body
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let total = entries.len();
    if from > total.max(1) {
        return Err(format!(
            "from_line pasa del final: la llamada tiene {total} líneas."
        ));
    }
    if from == 1 && text.chars().count() <= BUDGET_CHARS {
        return Ok(Page {
            text: text.to_string(),
            total,
            next: None,
        });
    }
    let mut out = String::new();
    if !header.is_empty() {
        out = header.join("\n");
        out.push_str("\n\n");
    }
    let mut used = out.chars().count();
    let mut next = None;
    for (i, entry) in entries.iter().enumerate().skip(from - 1) {
        let first = i + 1 == from;
        let cost = entry.chars().count() + if first { 0 } else { 2 };
        if !first && used + cost > BUDGET_CHARS {
            next = Some(i + 1);
            break;
        }
        if !first {
            out.push_str("\n\n");
        }
        out.push_str(entry);
        used += cost;
    }
    Ok(Page {
        text: out,
        total,
        next,
    })
}

/// La frase entera, o `SNIPPET_CHARS` caracteres alrededor de la primera vez que aparece `q` (ya
/// plegado), con `…` donde se cortó.
fn cut(text: &str, q: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= SNIPPET_CHARS {
        return text.to_string();
    }
    let (at, len) = locate(&chars, q).unwrap_or((0, 0));
    let start = at
        .saturating_sub(SNIPPET_CHARS.saturating_sub(len) / 2)
        .min(chars.len() - SNIPPET_CHARS);
    let end = start + SNIPPET_CHARS;
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// (primer carácter, cuántos) de `q` en `chars`, plegando carácter a carácter. None si así no
/// aparece (el plegado de un texto entero puede diferir en casos raros, como la sigma final).
fn locate(chars: &[char], q: &str) -> Option<(usize, usize)> {
    let mut folded = String::new();
    let mut owner = Vec::new(); // el carácter original de cada byte de `folded`
    let mut buf = [0u8; 4];
    for (i, c) in chars.iter().enumerate() {
        let f = fold(c.encode_utf8(&mut buf));
        owner.extend(std::iter::repeat_n(i, f.len()));
        folded.push_str(&f);
    }
    let b = folded.find(q).filter(|_| !q.is_empty())?;
    let first = owner[b];
    let last = owner[b + q.len() - 1];
    Some((first, last + 1 - first))
}

//! Pasar una llamada a Claude. "Copiar para Claude": la transcripción con una instrucción, para
//! pegarla en una conversación sin la conexión MCP. "Conectar con Claude Code": registrar
//! `Anotalo.exe mcp` con `claude mcp add`. Aquí solo la lógica; quién ejecuta `claude` y quién
//! copia al portapapeles lo pone la app.

use std::fs;
use std::path::{Path, PathBuf};

use crate::app::CALL_LABEL;
use crate::config::DEFAULT_CLAUDE_INSTRUCTION;
use crate::library::{Kind, parse_recording};
use crate::paths::Profile;

/// Lo que se muestra, como mucho, de lo que respondió `claude` al fallar.
const ERROR_CHARS: usize = 500;
/// Si no hay `PATHEXT`.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// El nombre del servidor en Claude Code. La versión de desarrollo usa otro para no pisar a la
/// instalada.
pub fn mcp_name(p: Profile) -> &'static str {
    match p {
        Profile::Installed => "anotalo",
        Profile::Dev => "anotalo-dev",
    }
}

/// `claude mcp add --scope user <name> -- <exe> mcp`, sin el `claude`.
pub fn add_args(name: &str, exe: &Path) -> Vec<String> {
    let exe = exe.to_string_lossy().into_owned();
    ["mcp", "add", "--scope", "user", name, "--", &exe, "mcp"]
        .map(String::from)
        .to_vec()
}

/// `claude mcp remove --scope user <name>`, sin el `claude`.
pub fn remove_args(name: &str) -> Vec<String> {
    ["mcp", "remove", "--scope", "user", name]
        .map(String::from)
        .to_vec()
}

/// La orden para escribirla a mano, con la ruta del exe entre comillas.
pub fn display_command(name: &str, exe: &Path) -> String {
    format!(
        "claude mcp add --scope user {name} -- \"{}\" mcp",
        exe.display()
    )
}

/// Dónde buscar `claude`: el PATH del usuario y el del equipo (del registro: una app que arrancó
/// con Windows tiene el PATH de entonces), el del proceso y las carpetas donde lo dejan el
/// instalador de Claude Code (`~\.local\bin`) y npm (`%APPDATA%\npm`). Con las `%VARIABLES%`
/// expandidas, sin entradas vacías y sin repetir (sin distinguir mayúsculas).
pub fn search_dirs(
    user_path: Option<String>,
    machine_path: Option<String>,
    process_path: Option<String>,
    home: &Path,
    appdata: &Path,
) -> Vec<PathBuf> {
    let listed = [user_path, machine_path, process_path]
        .into_iter()
        .flatten()
        .flat_map(|path| {
            path.split(';')
                .map(|e| expand(e.trim().trim_matches('"').trim()))
                .collect::<Vec<_>>()
        })
        .map(PathBuf::from);
    let known = [home.join(".local").join("bin"), appdata.join("npm")];
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for dir in listed.chain(known) {
        let key = dir
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase();
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(dir);
    }
    out
}

/// `%VAR%` → su valor, como `ExpandEnvironmentStrings`: una variable que no existe se queda tal
/// cual.
fn expand(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// El primer `claude<ext>` que existe: carpeta a carpeta y, en cada una, en el orden de
/// `PATHEXT` (como lo busca Windows).
pub fn find_claude(dirs: &[PathBuf], pathext: &str) -> Option<PathBuf> {
    let mut exts: Vec<String> = pathext
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| {
            let e = e.to_lowercase();
            if e.starts_with('.') {
                e
            } else {
                format!(".{e}")
            }
        })
        .collect();
    if exts.is_empty() {
        return find_claude(dirs, DEFAULT_PATHEXT);
    }
    exts.dedup();
    dirs.iter()
        .flat_map(|d| exts.iter().map(move |e| d.join(format!("claude{e}"))))
        .find(|p| p.is_file())
}

/// Cómo fue "Conectar con Claude Code".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connect {
    Connected,
    /// No está `claude`: se muestra la orden para copiarla.
    NotFound {
        command: String,
    },
    /// `claude` falló: su mensaje (recortado) y la orden.
    Failed {
        command: String,
        error: String,
    },
}

/// Ejecuta `claude` (la ruta) con esos argumentos: (código de salida, stdout + stderr), o por qué
/// no se pudo ejecutar.
pub type Runner<'a> = &'a dyn Fn(&Path, &[String]) -> Result<(i32, String), String>;

/// Registra `exe mcp` en Claude Code con el nombre `name`. Primero lo quita (da igual si no
/// estaba): `add` falla si el nombre ya existe, y así también se corrige un exe que cambió de
/// sitio.
pub fn connect(claude: Option<&Path>, name: &str, exe: &Path, run: Runner) -> Connect {
    let command = display_command(name, exe);
    let Some(claude) = claude else {
        return Connect::NotFound { command };
    };
    match run(claude, &remove_args(name)) {
        Ok((code, out)) => log::debug!("claude mcp remove: código {code} ({})", out.trim()),
        Err(e) => log::debug!("claude mcp remove no se pudo ejecutar ({e})"),
    }
    match run(claude, &add_args(name, exe)) {
        Ok((0, _)) => {
            log::info!("Conectado con Claude Code como {name}");
            Connect::Connected
        }
        Ok((code, out)) => {
            let out = clip(&out);
            let error = if out.is_empty() {
                format!("claude terminó con el código {code}.")
            } else {
                out
            };
            log::warn!("claude mcp add falló ({code}): {error}");
            Connect::Failed { command, error }
        }
        Err(e) => {
            log::warn!("claude mcp add no se pudo ejecutar ({e})");
            Connect::Failed {
                command,
                error: clip(&e),
            }
        }
    }
}

/// Sin espacios alrededor y como mucho `ERROR_CHARS` caracteres.
fn clip(s: &str) -> String {
    s.trim().chars().take(ERROR_CHARS).collect()
}

/// `MM:SS`, o `H:MM:SS` desde una hora (como la lista de la app).
fn duration(s: u32) -> String {
    let (h, rem) = (s / 3600, s % 3600);
    if h > 0 {
        format!("{h}:{:02}:{:02}", rem / 60, rem % 60)
    } else {
        format!("{:02}:{:02}", rem / 60, rem % 60)
    }
}

/// El texto de "Copiar para Claude": la instrucción (vacía = la de siempre), una línea en blanco y
/// la llamada dentro de `<transcripcion archivo="…">`: una cabecera (qué, cuándo, cuánto, título,
/// cliente, personas) y una línea por frase (`[HH:MM:SS] Persona: texto`) o marca
/// (`[HH:MM:SS] (Marca)`).
pub fn format_for_claude(md: &Path, instruction: &str) -> Result<String, String> {
    if let Err(e) = fs::read(md) {
        log::warn!("No se pudo leer {} ({e})", md.display());
        return Err("No se pudo leer la grabación.".into());
    }
    let rec = parse_recording(md);
    let instruction = match instruction.trim() {
        "" => DEFAULT_CLAUDE_INSTRUCTION,
        i => i,
    };
    let platform = match rec.app.as_str() {
        CALL_LABEL => "Manual", // grabada a mano
        a => a,
    };
    let mut head = match rec.kind {
        Kind::Note => "Nota rápida".to_string(),
        Kind::Call if platform.is_empty() => "Llamada".to_string(),
        Kind::Call => format!("Llamada: {platform}"),
    };
    if let Some(d) = rec.started_at {
        head += &format!(" — {}", d.format("%Y-%m-%d %H:%M"));
    }
    if let Some(s) = rec.duration_s {
        head += &format!(" ({})", duration(s));
    }
    let mut out = vec![
        instruction.to_string(),
        String::new(),
        format!("<transcripcion archivo=\"{}\">", rec.id),
        head,
    ];
    if let Some(t) = &rec.title {
        out.push(format!("Título: {t}"));
    }
    if let Some(c) = &rec.client {
        out.push(format!("Cliente: {c}"));
    }
    // Todas las personas, también el usuario: ninguna frase tiene el hablante vacío.
    let people = rec.people("");
    if !people.is_empty() {
        out.push(format!("Personas: {}", people.join(", ")));
    }
    out.push(String::new());
    out.extend(rec.lines.iter().map(|l| {
        if l.mark {
            format!("[{}] (Marca)", l.ts)
        } else {
            format!("[{}] {}: {}", l.ts, l.speaker, l.text)
        }
    }));
    out.push("</transcripcion>".into());
    Ok(out.join("\n"))
}

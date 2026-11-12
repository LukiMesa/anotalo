//! Editar la cabecera de una grabación ya guardada (spec §2.4.2): el cliente, en una línea
//! `- Cliente: …` que la app de Python ignora. Se escribe en un `.tmp` y se renombra, con los
//! saltos de línea que ya tenía el archivo, y nunca mientras se graba.

use std::fs;
use std::path::Path;

use crate::speaker::naming::being_recorded;

const MAX_CLIENT_CHARS: usize = 80;
const CLIENT: &str = "- Cliente:";
const TITLE: &str = "- Título:";
const TIMES: [&str; 3] = ["- Inicio:", "- Fin:", "- Duración:"];

/// El cliente sin espacios alrededor ("" = sin cliente). Sin caracteres de control ni
/// separadores de línea (partirían la cabecera) y de 80 caracteres como mucho.
pub fn validate_client(s: &str) -> Result<String, String> {
    let c = s.trim();
    if c.chars().count() > MAX_CLIENT_CHARS {
        return Err(format!(
            "Cliente no válido: más de {MAX_CLIENT_CHARS} caracteres"
        ));
    }
    if c.chars()
        .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return Err("Cliente no válido: no puede llevar saltos de línea".into());
    }
    Ok(c.to_string())
}

/// Pone (o cambia) el cliente de la grabación `md`; vacío lo quita. La línea va detrás de
/// `- Título:` si lo hay, y si no detrás de la última de Inicio/Fin/Duración.
pub fn set_client(md: &Path, client: &str) -> Result<(), String> {
    let client = validate_client(client)?;
    let file = md
        .file_name()
        .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    if being_recorded(md) {
        return Err("La llamada aún se está grabando; pon el cliente al colgar".into());
    }
    let text = fs::read_to_string(md).map_err(|e| format!("No se pudo leer {file}: {e}"))?;
    let out = with_client(&text, &client)
        .ok_or_else(|| format!("{file} no tiene la cabecera de una grabación"))?;
    if out == text {
        return Ok(());
    }
    let tmp = md.with_extension("md.tmp");
    fs::write(&tmp, out)
        .and_then(|()| fs::rename(&tmp, md))
        .map_err(|e| format!("No se pudo guardar {file}: {e}"))
}

/// El salto con el que acaba una línea de `split_inclusive('\n')`.
fn eol(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn content(line: &str) -> &str {
    &line[..line.len() - eol(line).len()]
}

/// El texto con el cliente puesto (o quitado si es ""). None si hay que añadirlo y no hay una
/// cabecera de grabación donde ponerlo.
fn with_client(text: &str, client: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    // la cabecera llega hasta la primera línea "---" (como al leerla)
    let header = lines
        .iter()
        .position(|l| content(l).trim_end() == "---")
        .unwrap_or(lines.len());
    let is_client = |i: usize| i < header && content(lines[i]).starts_with(CLIENT);
    let client_line = |end: &str| format!("{CLIENT} {client}{end}");
    let mut out = String::with_capacity(text.len() + client.len() + 16);
    if client.is_empty() || (0..header).any(is_client) {
        // se cambia la primera línea de cliente y se quitan las demás
        let mut done = client.is_empty();
        for (i, l) in lines.iter().enumerate() {
            if !is_client(i) {
                out.push_str(l);
            } else if !done {
                out.push_str(&client_line(eol(l)));
                done = true;
            }
        }
        return Some(out);
    }
    let last = |prefixes: &[&str]| {
        (0..header)
            .rev()
            .find(|&i| prefixes.iter().any(|p| content(lines[i]).starts_with(p)))
    };
    let anchor = last(&[TITLE]).or_else(|| last(&TIMES))?;
    for (i, l) in lines.iter().enumerate() {
        out.push_str(l);
        if i == anchor {
            match eol(l) {
                "" => {
                    // la cabecera acaba sin salto de línea: se usa el del archivo
                    out.push_str(if text.contains("\r\n") { "\r\n" } else { "\n" });
                    out.push_str(&client_line(""));
                }
                end => out.push_str(&client_line(end)),
            }
        }
    }
    Some(out)
}

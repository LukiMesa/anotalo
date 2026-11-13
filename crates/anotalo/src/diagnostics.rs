//! "Copiar diagnóstico" (spec §4.6): lo que va detrás de la versión y los ajustes, que escribe
//! `UiApi`: el audio de ese momento, lo que el registro del micrófono marca en uso y el final de
//! app.log. Nunca títulos de ventanas ni lo dicho en las llamadas.

use std::path::Path;

/// Cuántas líneas del final de app.log.
pub const LOG_LINES: usize = 150;

/// El audio en el momento de copiar, ya en texto (como en app.log).
pub struct Audio {
    /// Quién tiene sonido abierto en la salida.
    pub render: String,
    /// Quién graba de un micrófono.
    pub capture: String,
    /// Las apps que el registro del micrófono (ConsentStore) marca en uso.
    pub consent: String,
}

/// Las secciones "Audio", "Micrófono en uso" y "Registro" para `log` (app.log).
pub fn collect(log: &Path, audio: impl FnOnce() -> Audio) -> String {
    let a = audio();
    let tail = match std::fs::read(log) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let kept: Vec<&str> = text.lines().filter(|l| !private(l)).collect();
            kept[kept.len().saturating_sub(LOG_LINES)..].join("\n")
        }
        Err(e) => format!("(no se pudo leer app.log: {e})"),
    };
    format!(
        "## Audio\nSonando: {}\nGrabando: {}\n\n## Micrófono en uso\nConsentStore en uso: {}\n\n\
         ## Registro (últimas {LOG_LINES} líneas de app.log)\n{tail}\n",
        a.render, a.capture, a.consent
    )
}

/// Una línea de app.log que no se copia: títulos de ventanas (pestañas, asuntos de correos) o lo
/// dicho en una llamada (con el registro detallado, `[00:01:15] Ana: …`).
fn private(line: &str) -> bool {
    // "2026-11-12 10:00:00,123 INFO anotalo::x: mensaje"
    let msg = line.split_once(": ").map_or(line, |(_, m)| m);
    msg.starts_with("Ventanas de ") || msg.starts_with("Las demás ventanas de ") || spoken(msg)
}

/// `[hh:mm:ss] ` al principio.
fn spoken(msg: &str) -> bool {
    let b = msg.as_bytes();
    b.len() > 11
        && b[0] == b'['
        && b[9] == b']'
        && b[1..9].iter().enumerate().all(|(i, c)| {
            if i == 2 || i == 5 {
                *c == b':'
            } else {
                c.is_ascii_digit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio() -> Audio {
        Audio {
            render: "Teams.exe (Altavoces, activa)".into(),
            capture: "Teams.exe (Micrófono, activa)".into(),
            consent: "[\"Teams\"]".into(),
        }
    }

    fn line(n: usize, msg: &str) -> String {
        format!(
            "2026-11-12 10:00:{:02},{n:03} INFO anotalo::tray_app: {msg}",
            n % 60
        )
    }

    #[test]
    fn test_diagnostics_has_sections_and_log_tail() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("app.log");
        let mut lines: Vec<String> = (1..=400).map(|n| line(n, &format!("línea {n}"))).collect();
        // títulos de ventanas y frases de una llamada (con el registro detallado): no salen
        lines.insert(
            390,
            line(
                0,
                "Ventanas de Teams: [\"Reunión con Acme | Microsoft Teams\"] y 2 más",
            ),
        );
        lines.insert(
            391,
            line(0, "Las demás ventanas de Chrome: [\"Banco - Movimientos\"]"),
        );
        lines.insert(
            392,
            line(0, "[00:01:15] Ana: el presupuesto es de diez mil"),
        );
        std::fs::write(&log, lines.join("\n") + "\n").unwrap();

        let text = collect(&log, audio);
        for h in ["## Audio", "## Micrófono en uso", "## Registro"] {
            assert!(text.contains(h), "falta {h}: {text}");
        }
        assert!(
            !text.contains("## Versión") && !text.contains("## Ajustes"),
            "esas las pone UiApi"
        );
        assert!(text.contains("Sonando: Teams.exe (Altavoces, activa)"));
        assert!(text.contains("Grabando: Teams.exe (Micrófono, activa)"));
        assert!(text.contains("ConsentStore en uso: [\"Teams\"]"));
        let tail: Vec<&str> = text
            .split("## Registro")
            .nth(1)
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("2026-"))
            .collect();
        assert_eq!(tail.len(), LOG_LINES);
        assert!(tail[0].ends_with("línea 251"), "{}", tail[0]);
        assert!(tail[LOG_LINES - 1].ends_with("línea 400"));
        assert!(
            !text.contains("Ventanas de") && !text.contains("ventanas de"),
            "{text}"
        );
        assert!(!text.contains("presupuesto"));
    }

    #[test]
    fn test_diagnostics_without_log() {
        let d = tempfile::tempdir().unwrap();
        let text = collect(&d.path().join("app.log"), audio);
        assert!(text.contains("## Registro"));
        assert!(text.contains("(no se pudo leer app.log"), "{text}");
    }
}

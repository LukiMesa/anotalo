//! Editar la cabecera de una grabación ya guardada: el cliente.
use std::path::{Path, PathBuf};

use anotalo_core::meta::{set_client, validate_client};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Una copia de la fixture en una carpeta temporal: (carpeta, ruta, contenido original).
fn copy(name: &str) -> (tempfile::TempDir, PathBuf, Vec<u8>) {
    let d = tempfile::tempdir().unwrap();
    let original = std::fs::read(fixture(name)).unwrap();
    let p = d.path().join("2026-11-12_1000_Teams.md");
    std::fs::write(&p, &original).unwrap();
    (d, p, original)
}

fn read(p: &Path) -> String {
    String::from_utf8(std::fs::read(p).unwrap()).unwrap()
}

/// `original` con `line` añadida justo después de `after`.
fn inserted(original: &[u8], after: &str, line: &str) -> String {
    let text = String::from_utf8(original.to_vec()).unwrap();
    assert!(text.contains(after));
    text.replacen(after, &format!("{after}{line}"), 1)
}

#[test]
fn test_set_client_adds_replaces_and_removes() {
    // con título: va detrás del título
    let (_d, p, original) = copy("transcript_rust_title_marks.md");
    set_client(&p, "Acme Corp").unwrap();
    let title = "- Título: Reunión con Acme\n";
    assert_eq!(
        read(&p),
        inserted(&original, title, "- Cliente: Acme Corp\n")
    );
    set_client(&p, "  Globex  ").unwrap();
    assert_eq!(read(&p), inserted(&original, title, "- Cliente: Globex\n"));
    set_client(&p, "   ").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), original);
    set_client(&p, "").unwrap(); // quitar lo que no está: nada cambia
    assert_eq!(std::fs::read(&p).unwrap(), original);

    // sin título: detrás de la última de Inicio/Fin/Duración
    let (_d, p, original) = copy("transcript_rust_speakers.md");
    set_client(&p, "Ñandú Logística").unwrap();
    assert_eq!(
        read(&p),
        inserted(
            &original,
            "- Duración: 00:01:00\n",
            "- Cliente: Ñandú Logística\n"
        )
    );
    set_client(&p, "").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), original);

    // a medio escribir (solo el inicio)
    let (_d, p, original) = copy("library/2026-11-11_0905_Meet.md");
    set_client(&p, "Acme").unwrap();
    assert_eq!(
        read(&p),
        inserted(&original, "- Inicio: 09:05:07\n", "- Cliente: Acme\n")
    );
    assert!(!p.with_extension("md.tmp").exists());
}

#[test]
fn test_set_client_replaces_the_existing_line() {
    let (_d, p, original) = copy("library/2026-11-12_1000_Teams.md");
    set_client(&p, "Globex").unwrap();
    let text = String::from_utf8(original.clone()).unwrap();
    assert_eq!(
        read(&p),
        text.replace("- Cliente: Acme\n", "- Cliente: Globex\n")
    );
    set_client(&p, "").unwrap();
    assert_eq!(read(&p), text.replace("- Cliente: Acme\n", ""));
}

#[test]
fn test_set_client_keeps_crlf() {
    let (_d, p, original) = copy("library/2026-10-23_1504_Teams.md");
    set_client(&p, "Pérez y Asociados").unwrap();
    assert_eq!(
        read(&p),
        inserted(
            &original,
            "- Duración: 00:36:51\r\n",
            "- Cliente: Pérez y Asociados\r\n"
        )
    );
    set_client(&p, "").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), original);
}

#[test]
fn test_set_client_refused_while_recording() {
    let (_d, p, original) = copy("transcript_rust_speakers.md");
    let writer = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    let e = set_client(&p, "Acme").unwrap_err();
    assert!(e.contains("grabando"), "{e}");
    drop(writer);
    assert_eq!(std::fs::read(&p).unwrap(), original);
}

#[test]
fn test_set_client_rejects_line_breaks() {
    let (_d, p, original) = copy("transcript_rust_speakers.md");
    for bad in [
        "Acme\nX",
        "Acme\r\nX",
        "Acme\rX",
        "A\u{2028}B",
        "A\u{2029}B",
        "A\u{85}B",
        "A\tB",
    ] {
        assert!(set_client(&p, bad).is_err(), "{bad:?}");
        assert!(validate_client(bad).is_err(), "{bad:?}");
    }
    let long = "x".repeat(81);
    assert!(set_client(&p, &long).unwrap_err().contains("80"));
    assert_eq!(std::fs::read(&p).unwrap(), original);
    assert_eq!(validate_client(&"ñ".repeat(80)).unwrap(), "ñ".repeat(80));
    assert_eq!(validate_client("  Acme  ").unwrap(), "Acme");
    assert_eq!(validate_client("  ").unwrap(), "");
}

#[test]
fn test_set_client_needs_a_header() {
    let (_d, p, original) = copy("library/basura.md");
    assert!(set_client(&p, "Acme").is_err());
    assert_eq!(std::fs::read(&p).unwrap(), original);
    let missing = p.with_file_name("no-existe.md");
    assert!(set_client(&missing, "Acme").is_err());
}

/// Si no se puede cambiar el archivo (aquí, de solo lectura; en la vida real, OneDrive o un
/// antivirus), queda como estaba y no se deja un `.md.tmp` en la carpeta de grabaciones.
#[test]
fn test_failed_save_leaves_no_tmp() {
    let (d, p, original) = copy("transcript_rust_speakers.md");
    let writable = std::fs::metadata(&p).unwrap().permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&p, read_only).unwrap();
    let e = set_client(&p, "Acme").unwrap_err();
    std::fs::set_permissions(&p, writable).unwrap();
    assert!(e.contains("No se pudo guardar"), "{e}");
    assert_eq!(std::fs::read(&p).unwrap(), original);
    let names: Vec<_> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [p.file_name().unwrap()], "sin restos");
}

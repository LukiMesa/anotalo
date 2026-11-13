//! Port de tests/test_library.py (16 casos) más título, cliente, marcas, personas y carpetas
//! grandes.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anotalo_core::library::{
    Kind, Trash, fold, list_recordings, parse_recording, to_plain_text, to_readable,
};
use chrono::{NaiveDate, NaiveDateTime};

const CALL: &str = "# Llamada — Teams — 2026-10-23 15:04

- Inicio: 15:04:12
- Fin: 15:41:03
- Duración: 00:36:51

---

**[00:00:05] Yo:** Hola, ¿qué tal?

**[00:00:09] Otros:** Necesito cambiar el botón de contacto.

";

const NOTE: &str = "# Llamada — Nota — 2026-10-23 11:20

- Inicio: 11:20:00
- Fin: 11:21:48
- Duración: 00:01:48

---

**[00:00:01] Yo:** Hola

**[00:00:04] Yo:** Segunda frase

";

const HALF: &str = "# Llamada — Meet — 2026-10-22 17:45

- Inicio: 17:45:00

---

";

fn legacy() -> String {
    NOTE.replace("Nota", "Manual")
}

fn at(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, mi, s)
        .unwrap()
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

fn folder() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "2026-10-23_1504_Teams.md", CALL);
    write(d.path(), "2026-10-23_1120_Nota.md", NOTE);
    write(d.path(), "2026-10-21_0900_Manual.md", &legacy());
    write(d.path(), "2026-10-22_1745_Meet.md", HALF);
    write(d.path(), "basura.md", "\x00\x01 nada que ver");
    d
}

fn ids(folder: &Path, kind: &str, query: &str, client: Option<&str>) -> Vec<String> {
    list_recordings(folder, kind, query, client)
        .into_iter()
        .map(|r| r.id)
        .collect()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

#[test]
fn test_parse_call() {
    let f = folder();
    let r = parse_recording(&f.path().join("2026-10-23_1504_Teams.md"));
    assert!(r.app == "Teams" && r.kind == Kind::Call);
    assert_eq!(r.started_at, Some(at(2026, 10, 23, 15, 4, 12)));
    assert_eq!(r.duration_s, Some(2211));
    let speakers: Vec<&str> = r.lines.iter().map(|l| l.speaker.as_str()).collect();
    assert_eq!(speakers, ["Yo", "Otros"]);
    assert!(r.lines[1].ts == "00:00:09" && r.lines[1].start_s == 9);
}

#[test]
fn test_manual_is_note() {
    let f = folder();
    assert_eq!(
        parse_recording(&f.path().join("2026-10-21_0900_Manual.md")).kind,
        Kind::Note
    );
    assert_eq!(
        parse_recording(&f.path().join("2026-10-23_1120_Nota.md")).kind,
        Kind::Note
    );
}

#[test]
fn test_half_written_listed() {
    let f = folder();
    let r = parse_recording(&f.path().join("2026-10-22_1745_Meet.md"));
    assert!(r.ended_at.is_none() && r.duration_s.is_none() && r.lines.is_empty());
}

#[test]
fn test_garbage_listed_not_raise() {
    let f = folder();
    let r = parse_recording(&f.path().join("basura.md"));
    assert_eq!(r.id, "basura.md");
    assert!(r.lines.is_empty());
}

#[test]
fn test_list_sorted_newest_first() {
    let f = folder();
    let ids = ids(f.path(), "all", "", None);
    assert_eq!(
        ids[..4],
        [
            "2026-10-23_1504_Teams.md",
            "2026-10-23_1120_Nota.md",
            "2026-10-22_1745_Meet.md",
            "2026-10-21_0900_Manual.md",
        ]
    );
}

#[test]
fn test_filter_kind() {
    let f = folder();
    let apps = |kind: &str| {
        let mut a: Vec<String> = list_recordings(f.path(), kind, "", None)
            .into_iter()
            .map(|r| r.app)
            .collect();
        a.sort();
        a
    };
    assert_eq!(apps("note"), ["Manual", "Nota"]);
    // el archivo sin formato se lista como llamada sin app (datos parciales)
    assert_eq!(apps("call"), ["", "Meet", "Teams"]);
}

#[test]
fn test_search_accent_insensitive() {
    let f = folder();
    assert_eq!(
        ids(f.path(), "all", "boton", None),
        ["2026-10-23_1504_Teams.md"]
    );
    assert_eq!(
        ids(f.path(), "all", "QUÉ TAL", None),
        ["2026-10-23_1504_Teams.md"]
    );
}

#[test]
fn test_plain_text_note() {
    let f = folder();
    assert_eq!(
        to_plain_text(&parse_recording(&f.path().join("2026-10-23_1120_Nota.md"))),
        "Hola\n\nSegunda frase"
    );
}

#[test]
fn test_readable_format() {
    let f = folder();
    let text = to_readable(&parse_recording(&f.path().join("2026-10-23_1504_Teams.md")));
    assert_eq!(
        text.lines().next().unwrap(),
        "[00:00:05] Yo: Hola, ¿qué tal?"
    );
    assert!(!text.contains("**") && !text.contains('#'));
}

#[test]
fn test_trash_undo_roundtrip() {
    let f = folder();
    let t = Trash::new(f.path());
    let p = f.path().join("2026-10-23_1120_Nota.md");
    let token = t.delete(&p).unwrap();
    assert!(!p.exists());
    assert_eq!(t.undo(&token).unwrap(), p);
    assert!(p.exists());
}

#[test]
fn test_two_deletes_undo_each() {
    let f = folder();
    let t = Trash::new(f.path());
    let (a, b) = (
        f.path().join("2026-10-23_1120_Nota.md"),
        f.path().join("2026-10-23_1504_Teams.md"),
    );
    let (ta, tb) = (t.delete(&a).unwrap(), t.delete(&b).unwrap());
    Trash::new(f.path()).undo(&ta).unwrap(); // otra instancia (ventana reabierta)
    assert!(a.exists() && !b.exists());
    Trash::new(f.path()).undo(&tb).unwrap();
    assert!(b.exists());
}

#[test]
fn test_purge_old() {
    let f = folder();
    let t = Trash::new(f.path());
    let token = t.delete(&f.path().join("2026-10-23_1120_Nota.md")).unwrap();
    let name = token.split_once("__").unwrap().1;
    let old_token = format!("{}__{name}", now_ms() - 7_200_000);
    let trash = f.path().join(".papelera");
    std::fs::rename(trash.join(&token), trash.join(&old_token)).unwrap();
    let fresh = t
        .delete(&f.path().join("2026-10-23_1504_Teams.md"))
        .unwrap();
    assert_eq!(t.purge(3600.0).unwrap(), 1);
    assert!(!trash.join(&old_token).exists());
    assert!(trash.join(&fresh).exists());
}

#[test]
fn test_papelera_not_listed() {
    let f = folder();
    Trash::new(f.path())
        .delete(&f.path().join("2026-10-23_1120_Nota.md"))
        .unwrap();
    assert!(!ids(f.path(), "all", "", None).contains(&"2026-10-23_1120_Nota.md".to_string()));
}

#[test]
fn test_bad_values_never_empty_the_list() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "2026-10-23_1300_Teams.md",
        &CALL.replace("15:04:12", "25:00:00"),
    );
    write(d.path(), "2026-02-30_1000_Meet.md", HALF);
    write(d.path(), "2026-10-23_1120_Nota.md", NOTE);
    let mut got = ids(d.path(), "all", "", None);
    got.sort();
    assert_eq!(
        got,
        [
            "2026-02-30_1000_Meet.md",
            "2026-10-23_1120_Nota.md",
            "2026-10-23_1300_Teams.md",
        ]
    );
}

/// Un archivo de Anotalo con título y marcas se lee (en Python las marcas no son líneas: aquí sí,
/// y se comparan las frases).
#[test]
fn test_reads_anotalo_title_and_marks() {
    let rec = parse_recording(&fixture("transcript_rust_title_marks.md"));
    assert!(rec.kind == Kind::Call && rec.app == "Teams");
    let said: Vec<(&str, &str)> = rec
        .lines
        .iter()
        .filter(|l| !l.mark)
        .map(|l| (l.speaker.as_str(), l.text.as_str()))
        .collect();
    assert_eq!(
        said,
        [("Otros", "Hola, ¿empezamos?"), ("Yo", "Sí, adelante.")]
    );
    assert_eq!(rec.duration_s, Some(53));
}

#[test]
fn test_reads_anotalo_named_speakers() {
    let rec = parse_recording(&fixture("transcript_rust_speakers.md"));
    let speakers: Vec<&str> = rec.lines.iter().map(|l| l.speaker.as_str()).collect();
    assert_eq!(speakers, ["María", "Yo", "Persona 1", "María"]);
    assert_eq!(rec.duration_s, Some(60));
}

#[test]
fn test_title_client_marks_and_people() {
    let rec = parse_recording(&fixture("transcript_rust_title_marks.md"));
    assert_eq!(rec.title.as_deref(), Some("Reunión con Acme"));
    assert_eq!(rec.client, None);
    let marks: Vec<(&str, u32, &str, &str)> = rec
        .lines
        .iter()
        .filter(|l| l.mark)
        .map(|l| {
            (
                l.ts.as_str(),
                l.start_s,
                l.speaker.as_str(),
                l.text.as_str(),
            )
        })
        .collect();
    assert_eq!(marks, [("00:00:02", 2, "", "")]);
    assert_eq!(rec.lines.len(), 3);
    assert_eq!(rec.people("Yo"), ["Otros"]);
    assert_eq!(rec.snippet("Yo"), "Hola, ¿empezamos?");

    let rec = parse_recording(&fixture("transcript_rust_speakers.md"));
    assert_eq!(rec.title, None);
    assert_eq!(rec.people("Yo"), ["María", "Persona 1"]);

    let rec = parse_recording(&fixture("library/2026-11-12_1000_Teams.md"));
    assert_eq!(rec.title.as_deref(), Some("Revisión de la web"));
    assert_eq!(rec.client.as_deref(), Some("Acme"));
    assert_eq!(rec.people("Yo"), ["María", "Persona 1"]);
    assert_eq!(rec.lines.iter().filter(|l| l.mark).count(), 1);
    assert_eq!(rec.duration_s, Some(60));
    let readable = to_readable(&rec);
    assert!(readable.contains("\n[00:00:05] Marca\n"), "{readable}");
    assert_eq!(
        to_plain_text(&rec),
        "Hola, soy María.\n\nEncantado.\n\nY yo Juan.\n\nEmpezamos."
    );
}

/// La carpeta de ejemplo: escritos por Python (CRLF), por Anotalo, a medias y basura.
#[test]
fn test_fixture_folder_lists_every_file() {
    let recs = list_recordings(&fixture("library"), "all", "", None);
    let ids: Vec<&str> = recs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "2026-11-12_1000_Teams.md",
            "2026-11-11_0905_Meet.md",
            "2026-11-10_1120_Nota.md",
            "2026-10-23_1504_Teams.md",
            "basura.md",
        ]
    );
    let crlf = &recs[3];
    assert_eq!(crlf.duration_s, Some(2211));
    assert_eq!(crlf.lines[1].text, "Necesito cambiar el botón de contacto.");
    let half = &recs[1];
    assert!(half.ended_at.is_none() && half.lines.len() == 1);
    assert!(recs[4].lines.is_empty() && recs[4].started_at.is_none());
}

#[test]
fn test_query_matches_title_and_client() {
    let f = folder();
    let titled = CALL.replace(
        "- Duración: 00:36:51\n",
        "- Duración: 00:36:51\n- Título: Revisión de presupuestos\n",
    );
    write(f.path(), "2026-10-24_1000_Teams.md", &titled);
    let client = NOTE.replace(
        "- Duración: 00:01:48\n",
        "- Duración: 00:01:48\n- Cliente: Ñandú Logística\n",
    );
    write(f.path(), "2026-10-24_1100_Nota.md", &client);
    assert_eq!(
        ids(f.path(), "all", "revision", None),
        ["2026-10-24_1000_Teams.md"]
    );
    assert_eq!(
        ids(f.path(), "all", " NANDU ", None),
        ["2026-10-24_1100_Nota.md"]
    );
    assert_eq!(ids(f.path(), "note", "logistica", None).len(), 1);
    assert!(ids(f.path(), "call", "logistica", None).is_empty());
}

#[test]
fn test_client_filter_ignores_case_and_accents() {
    let d = tempfile::tempdir().unwrap();
    let with = |client: &str| {
        CALL.replace(
            "- Duración: 00:36:51\n",
            &format!("- Duración: 00:36:51\n- Cliente: {client}\n"),
        )
    };
    write(
        d.path(),
        "2026-10-23_1504_Teams.md",
        &with("Pérez y Asociados"),
    );
    write(d.path(), "2026-10-24_1504_Teams.md", &with("Acme"));
    write(d.path(), "2026-10-25_1504_Teams.md", CALL);
    assert_eq!(
        ids(d.path(), "all", "", Some("PEREZ y asociados")),
        ["2026-10-23_1504_Teams.md"]
    );
    assert_eq!(
        ids(d.path(), "all", "", Some("ácme")),
        ["2026-10-24_1504_Teams.md"]
    );
    assert!(ids(d.path(), "all", "", Some("Acm")).is_empty());
    assert_eq!(ids(d.path(), "all", "", Some("  ")).len(), 3);
    assert_eq!(ids(d.path(), "all", "", None).len(), 3);
    assert_eq!(fold("Ñandú ÁCME"), "nandu acme");
}

#[test]
fn test_large_folder_lists_fast() {
    let d = tempfile::tempdir().unwrap();
    let body: String = (0..200)
        .map(|k| {
            format!(
                "**[00:{:02}:{:02}] {}:** Frase número {k} de la llamada, con algo de texto para que pese.\n\n",
                k / 60,
                k % 60,
                if k % 2 == 0 { "Yo" } else { "Otros" }
            )
        })
        .collect();
    for i in 0..500 {
        let name = format!(
            "2025-{:02}-{:02}_{:02}{:02}_Teams.md",
            1 + i % 12,
            1 + i % 28,
            i % 24,
            i % 60
        );
        write(d.path(), &name, &format!("{CALL}{body}"));
    }
    let t = Instant::now();
    let all = list_recordings(d.path(), "all", "", None);
    let listed = t.elapsed();
    assert_eq!(all.len(), 500);
    assert!(all.iter().all(|r| r.lines.len() == 202));
    let t = Instant::now();
    assert!(list_recordings(d.path(), "all", "inexistente", None).is_empty());
    let searched = t.elapsed();
    // holgado: la prueba corre junto a otras compilaciones y en máquinas de CI lentas; lo que se
    // vigila es que no se congele (antes de optimizar tardaba decenas de segundos)
    assert!(listed < Duration::from_secs(3), "{listed:?}");
    assert!(searched < Duration::from_secs(3), "{searched:?}");
}

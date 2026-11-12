//! Poner nombre a una persona en una llamada ya grabada, y las huellas de cada llamada.
mod common {
    pub mod speaker;
}

use std::path::Path;
use std::time::{Duration, SystemTime};

use anotalo_core::speaker::diarizer::CallVoice;
use anotalo_core::speaker::memory::{PRINT_SPACE, VoiceStore};
use anotalo_core::speaker::naming::{
    CallPrints, KEEP_CALL_PRINTS, call_prints_path, forget_in_call_prints, name_in_call,
    prune_call_prints, rename_speaker, save_call_prints,
};
use common::speaker::basis;

const MD: &str = "# Llamada — Teams — 2026-11-12 10:00\n\n- Inicio: 10:00:00\n\n---\n\n\
**[00:00:01] Persona 1:** Hola.\n\n**[00:00:03] Persona 10:** Otra.\n\n\
**[00:00:05] Yo:** Dijo Persona 1: que sí.\n\n**[00:00:07] Persona 1:** Vale.\n\n";

/// Las etiquetas de la app (`label_me`, `label_others`): no se guardan como voces.
const RESERVED: &[&str] = &["Yo", "Otros"];

fn write_prints(prints_dir: &Path, md: &Path, voices: &[(&str, Vec<f32>, f64)]) {
    let voices = voices
        .iter()
        .map(|(l, p, s)| CallVoice {
            label: l.to_string(),
            print: p.clone(),
            seconds: *s,
            named: false,
        })
        .collect();
    save_call_prints(prints_dir, md, voices).unwrap();
}

fn read_prints(prints_dir: &Path, md: &Path) -> CallPrints {
    serde_json::from_slice(&std::fs::read(call_prints_path(prints_dir, md)).unwrap()).unwrap()
}

fn days_ago(days: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(days * 86_400)
}

fn set_mtime(p: &Path, t: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// Si la fecha de modificación de `p` es (casi) `t`.
fn modified_at(p: &Path, t: SystemTime) -> bool {
    let m = std::fs::metadata(p).unwrap().modified().unwrap();
    let diff = m.duration_since(t).unwrap_or_else(|e| e.duration());
    diff < Duration::from_secs(2)
}

#[test]
fn test_rename_changes_only_that_speaker() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.md");
    std::fs::write(&p, MD).unwrap();
    assert_eq!(rename_speaker(&p, "Persona 1", "María").unwrap(), 2);
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        MD.replace("] Persona 1:**", "] María:**")
    );
}

#[test]
fn test_rename_keeps_crlf_files() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.md");
    std::fs::write(&p, MD.replace('\n', "\r\n")).unwrap();
    assert_eq!(rename_speaker(&p, "Persona 1", "María").unwrap(), 2);
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        MD.replace("] Persona 1:**", "] María:**")
            .replace('\n', "\r\n")
    );
}

#[test]
fn test_rename_rejects_bad_names_and_missing_labels() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.md");
    std::fs::write(&p, MD).unwrap();
    let long = "a".repeat(41);
    for bad in [
        "",
        "  ",
        "a:b",
        "**x",
        "[x]",
        "marca",
        "x\ny",
        "a\u{2028}b",
        "a\u{0b}b",
        "a\tb",
        long.as_str(),
    ] {
        assert!(rename_speaker(&p, "Persona 1", bad).is_err(), "{bad:?}");
    }
    assert!(rename_speaker(&p, "Persona 3", "Ana").is_err());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), MD);
}

#[test]
fn test_name_in_call_remembers_the_voice() {
    let d = tempfile::tempdir().unwrap();
    let (md, prints) = (d.path().join("a.md"), d.path().join("llamadas"));
    std::fs::write(&md, MD).unwrap();
    write_prints(
        &prints,
        &md,
        &[("Persona 1", basis(0), 20.0), ("Persona 10", basis(1), 5.0)],
    );
    let voces = d.path().join("voces.json");
    let mut store = VoiceStore::load(&voces);
    let out = name_in_call(&md, &prints, &mut store, "Persona 1", "María", RESERVED).unwrap();
    assert_eq!((out.lines, out.remembered), (2, true));
    let saved = VoiceStore::load(&voces);
    assert_eq!(
        (saved.voices[0].name.as_str(), saved.voices[0].seconds),
        ("María", 20.0)
    );
    let again = name_in_call(&md, &prints, &mut store, "María", "María P.", RESERVED).unwrap();
    assert!(again.remembered);
    let none = name_in_call(&md, &prints, &mut store, "Yo", "Lucas", RESERVED).unwrap();
    assert_eq!((none.lines, none.remembered), (1, false));
}

#[test]
fn test_naming_again_with_the_same_name_does_not_count_twice() {
    let d = tempfile::tempdir().unwrap();
    let (md, prints) = (d.path().join("a.md"), d.path().join("llamadas"));
    std::fs::write(&md, MD).unwrap();
    write_prints(&prints, &md, &[("Persona 1", basis(0), 20.0)]);
    let voces = d.path().join("voces.json");
    let mut store = VoiceStore::load(&voces);
    name_in_call(&md, &prints, &mut store, "Persona 1", "María", RESERVED).unwrap();
    let again = name_in_call(&md, &prints, &mut store, "María", "María", RESERVED).unwrap();
    assert_eq!((again.lines, again.remembered), (2, true));
    assert_eq!(store.voices[0].seconds, 20.0);
    assert_eq!(VoiceStore::load(&voces).voices[0].seconds, 20.0);
    // sin esa voz en la memoria (p. ej. si voces.json se perdió), volver a nombrarla la recuerda
    let mut fresh = VoiceStore::load(&d.path().join("otras.json"));
    let out = name_in_call(&md, &prints, &mut fresh, "María", "María", RESERVED).unwrap();
    assert!(out.remembered);
    assert_eq!(fresh.voices[0].seconds, 20.0);
}

#[test]
fn test_naming_keeps_the_prints_file_age() {
    let d = tempfile::tempdir().unwrap();
    let (md, prints) = (d.path().join("a.md"), d.path().join("llamadas"));
    std::fs::write(&md, MD).unwrap();
    write_prints(&prints, &md, &[("Persona 1", basis(0), 20.0)]);
    let file = call_prints_path(&prints, &md);
    let t = days_ago(20);
    set_mtime(&file, t);
    let mut store = VoiceStore::load(&d.path().join("voces.json"));
    name_in_call(&md, &prints, &mut store, "Persona 1", "María", RESERVED).unwrap();
    assert_eq!(read_prints(&prints, &md).voices[0].label, "María");
    assert!(modified_at(&file, t));
}

#[test]
fn test_forget_in_call_prints() {
    let d = tempfile::tempdir().unwrap();
    let prints = d.path().join("llamadas");
    let (a, b, c) = (
        d.path().join("a.md"),
        d.path().join("b.md"),
        d.path().join("c.md"),
    );
    write_prints(
        &prints,
        &a,
        &[("María", basis(0), 20.0), ("Persona 2", basis(1), 5.0)],
    );
    write_prints(&prints, &b, &[("María", basis(0), 8.0)]);
    write_prints(&prints, &c, &[("Persona 1", basis(2), 9.0)]);
    let t = days_ago(10);
    set_mtime(&call_prints_path(&prints, &a), t);
    assert_eq!(forget_in_call_prints(&prints, "María"), 2);
    let left = read_prints(&prints, &a).voices;
    assert_eq!(
        left.iter().map(|v| v.label.as_str()).collect::<Vec<_>>(),
        ["Persona 2"]
    );
    assert!(modified_at(&call_prints_path(&prints, &a), t));
    assert!(!call_prints_path(&prints, &b).exists());
    assert_eq!(read_prints(&prints, &c).voices[0].label, "Persona 1");
    assert_eq!(forget_in_call_prints(&prints, "María"), 0);
    assert_eq!(
        forget_in_call_prints(&d.path().join("no-existe"), "María"),
        0
    );
}

#[test]
fn test_prints_from_another_space_are_not_used() {
    let d = tempfile::tempdir().unwrap();
    let (md, prints) = (d.path().join("a.md"), d.path().join("llamadas"));
    std::fs::write(&md, MD).unwrap();
    write_prints(&prints, &md, &[("Persona 1", basis(0), 20.0)]);
    let file = call_prints_path(&prints, &md);
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(json["space"], PRINT_SPACE);
    json["space"] = "otro/modelo".into();
    std::fs::write(&file, serde_json::to_vec(&json).unwrap()).unwrap();
    let voces = d.path().join("voces.json");
    let mut store = VoiceStore::load(&voces);
    let out = name_in_call(&md, &prints, &mut store, "Persona 1", "María", RESERVED).unwrap();
    assert_eq!((out.lines, out.remembered), (2, false));
    assert!(store.voices.is_empty() && !voces.exists());
}

#[test]
fn test_prune_call_prints() {
    let d = tempfile::tempdir().unwrap();
    let (old, new) = (d.path().join("old.md.json"), d.path().join("new.md.json"));
    for p in [&old, &new] {
        std::fs::write(p, "{}").unwrap();
    }
    set_mtime(&old, days_ago(31));
    set_mtime(&new, days_ago(29));
    assert_eq!(prune_call_prints(d.path(), KEEP_CALL_PRINTS), 1);
    assert!(!old.exists() && new.exists());
    assert_eq!(
        prune_call_prints(&d.path().join("no-existe"), KEEP_CALL_PRINTS),
        0
    );
}

fn prints_of(prints_dir: &Path, md: &Path) -> CallPrints {
    serde_json::from_slice(&std::fs::read(call_prints_path(prints_dir, md)).unwrap()).unwrap()
}

/// Juntar a "Persona 10" con "Persona 1" (la misma persona partida en dos) corrige el archivo,
/// pero "Persona 1" no se guarda como voz: chocaría con las etiquetas de otras llamadas.
#[test]
fn test_reserved_label_renames_without_remembering() {
    let d = tempfile::tempdir().unwrap();
    let (md, prints) = (d.path().join("a.md"), d.path().join("llamadas"));
    std::fs::write(&md, MD).unwrap();
    write_prints(
        &prints,
        &md,
        &[("Persona 1", basis(0), 20.0), ("Persona 10", basis(1), 5.0)],
    );
    let voces = d.path().join("voces.json");
    let mut store = VoiceStore::load(&voces);
    let out = name_in_call(
        &md,
        &prints,
        &mut store,
        "Persona 10",
        "Persona 1",
        RESERVED,
    )
    .unwrap();
    assert_eq!((out.lines, out.remembered), (1, false));
    assert!(VoiceStore::load(&voces).voices.is_empty());
    let p = prints_of(&prints, &md);
    assert_eq!(p.voices.len(), 1);
    assert_eq!(
        (p.voices[0].label.as_str(), p.voices[0].seconds),
        ("Persona 1", 25.0)
    );
    let yo = name_in_call(&md, &prints, &mut store, "Persona 1", "otros", RESERVED).unwrap();
    assert!(!yo.remembered && store.voices.is_empty());
}

/// Corregir un nombre (María → María P.) mueve la voz: no quedan dos voces iguales.
#[test]
fn test_correcting_a_name_moves_the_voice() {
    let d = tempfile::tempdir().unwrap();
    let (md, prints) = (d.path().join("a.md"), d.path().join("llamadas"));
    std::fs::write(&md, MD).unwrap();
    write_prints(&prints, &md, &[("Persona 1", basis(0), 20.0)]);
    let voces = d.path().join("voces.json");
    let mut store = VoiceStore::load(&voces);
    name_in_call(&md, &prints, &mut store, "Persona 1", "María", RESERVED).unwrap();
    name_in_call(&md, &prints, &mut store, "María", "María P.", RESERVED).unwrap();
    let saved = VoiceStore::load(&voces);
    assert_eq!(saved.voices.len(), 1);
    assert_eq!(
        (saved.voices[0].name.as_str(), saved.voices[0].seconds),
        ("María P.", 20.0)
    );
}

/// Mientras se graba, la app tiene el archivo abierto y lo reescribe al colgar: no se toca.
#[test]
fn test_naming_a_call_still_recording_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.md");
    std::fs::write(&p, MD).unwrap();
    let writer = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    let e = rename_speaker(&p, "Persona 1", "María").unwrap_err();
    assert!(e.contains("grabando"), "{e}");
    drop(writer);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), MD);
}

#[test]
fn test_prune_removes_half_saved_prints() {
    let d = tempfile::tempdir().unwrap();
    let (stale, fresh) = (
        d.path().join("a.md.json.tmp"),
        d.path().join("b.md.json.tmp"),
    );
    for p in [&stale, &fresh] {
        std::fs::write(p, "{").unwrap();
    }
    set_mtime(&stale, days_ago(2));
    assert_eq!(prune_call_prints(d.path(), KEEP_CALL_PRINTS), 1);
    assert!(!stale.exists() && fresh.exists());
}

//! Poner nombre a una persona en una llamada ya grabada, y las huellas de cada llamada.
mod common {
    pub mod speaker;
}

use std::path::Path;

use anotalo_core::speaker::diarizer::CallVoice;
use anotalo_core::speaker::memory::VoiceStore;
use anotalo_core::speaker::naming::{
    KEEP_CALL_PRINTS, name_in_call, prune_call_prints, rename_speaker, save_call_prints,
};
use common::speaker::basis;

const MD: &str = "# Llamada — Teams — 2026-11-12 10:00\n\n- Inicio: 10:00:00\n\n---\n\n\
**[00:00:01] Persona 1:** Hola.\n\n**[00:00:03] Persona 10:** Otra.\n\n\
**[00:00:05] Yo:** Dijo Persona 1: que sí.\n\n**[00:00:07] Persona 1:** Vale.\n\n";

fn write_prints(prints_dir: &Path, md: &Path, voices: &[(&str, Vec<f32>, f64)]) {
    let voices = voices
        .iter()
        .map(|(l, p, s)| CallVoice {
            label: l.to_string(),
            print: p.clone(),
            seconds: *s,
        })
        .collect();
    save_call_prints(prints_dir, md, voices).unwrap();
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
    let out = name_in_call(&md, &prints, &mut store, "Persona 1", "María").unwrap();
    assert_eq!((out.lines, out.remembered), (2, true));
    let saved = VoiceStore::load(&voces);
    assert_eq!(
        (saved.voices[0].name.as_str(), saved.voices[0].seconds),
        ("María", 20.0)
    );
    let again = name_in_call(&md, &prints, &mut store, "María", "María P.").unwrap();
    assert!(again.remembered);
    let none = name_in_call(&md, &prints, &mut store, "Yo", "Lucas").unwrap();
    assert_eq!((none.lines, none.remembered), (1, false));
}

#[test]
fn test_prune_call_prints() {
    let d = tempfile::tempdir().unwrap();
    let (old, new) = (d.path().join("old.md.json"), d.path().join("new.md.json"));
    for p in [&old, &new] {
        std::fs::write(p, "{}").unwrap();
    }
    let ago =
        |days: u64| std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(ago(31))
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&new)
        .unwrap()
        .set_modified(ago(29))
        .unwrap();
    assert_eq!(prune_call_prints(d.path(), KEEP_CALL_PRINTS), 1);
    assert!(!old.exists() && new.exists());
    assert_eq!(
        prune_call_prints(&d.path().join("no-existe"), KEEP_CALL_PRINTS),
        0
    );
}

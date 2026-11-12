//! Memoria de voces: `voces.json` y la votación que pone nombre a un grupo (spec §2.4.5).
mod common {
    pub mod speaker;
}

use anotalo_core::speaker::embed::{DIM, cosine, l2n};
use anotalo_core::speaker::memory::{StoredVoice, VoiceStore, name_clusters};
use common::speaker::basis;

/// Un vector con coseno `cos` respecto a `basis(0)`.
fn at(cos: f32) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[0] = cos;
    v[1] = (1.0 - cos * cos).sqrt();
    v
}

fn maria() -> Vec<StoredVoice> {
    vec![StoredVoice {
        name: "María".into(),
        print: basis(0),
        seconds: 60.0,
    }]
}

fn named(n: &str) -> Option<String> {
    Some(n.to_string())
}

#[test]
fn test_cluster_named_when_most_long_segments_match() {
    let c = vec![
        (at(0.6), 3.0),
        (at(0.55), 2.5),
        (at(0.7), 4.0),
        (basis(5), 2.0),
    ];
    assert_eq!(name_clusters(&[c], &maria()), vec![named("María")]);
}

#[test]
fn test_cluster_not_named_below_threshold() {
    // 0,50 (no el 0,44 del spike): en audio limpio, 1 de cada 10 desconocidos salía con nombre
    let c = vec![(at(0.40), 3.0), (at(0.47), 3.0), (at(0.49), 3.0)];
    assert_eq!(name_clusters(&[c], &maria()), vec![None]);
}

#[test]
fn test_half_or_fewer_votes_is_not_enough() {
    // "la mayoría" (spec): la mitad no basta
    let half = vec![
        (at(0.6), 3.0),
        (at(0.6), 3.0),
        (basis(5), 3.0),
        (basis(6), 3.0),
    ];
    let more = vec![
        (at(0.6), 3.0),
        (at(0.6), 3.0),
        (at(0.6), 3.0),
        (basis(6), 3.0),
        (basis(7), 3.0),
    ];
    // cada grupo por separado (juntos, la regla de un nombre por llamada taparía el caso)
    assert_eq!(name_clusters(&[half], &maria()), vec![None]);
    assert_eq!(name_clusters(&[more], &maria()), vec![named("María")]);
}

#[test]
fn test_short_segments_vote_only_without_long_ones() {
    let mixed = vec![
        (at(0.6), 3.0),
        (at(0.7), 2.5),
        (basis(5), 1.0),
        (basis(6), 1.0),
        (basis(7), 1.0),
    ];
    let short = vec![(basis(5), 1.0), (at(0.6), 1.0), (basis(6), 1.0)];
    assert_eq!(
        name_clusters(&[mixed, short], &maria()),
        vec![named("María"), None]
    );
}

#[test]
fn test_a_single_voter_is_not_enough() {
    let one_long = vec![(at(0.9), 3.0), (basis(5), 1.0), (basis(6), 1.0)];
    let one_short = vec![(at(0.9), 1.0)];
    assert_eq!(
        name_clusters(&[one_long, one_short], &maria()),
        vec![None, None]
    );
}

#[test]
fn test_tie_between_names_is_not_named() {
    let mut voices = maria();
    voices.push(StoredVoice {
        name: "Juan".into(),
        print: basis(1),
        seconds: 60.0,
    });
    let towards_juan = |cos: f32| {
        let mut v = vec![0.0; DIM];
        v[1] = cos;
        v[2] = (1.0 - cos * cos).sqrt();
        v
    };
    let c = vec![
        (at(0.8), 3.0),
        (at(0.8), 3.0),
        (towards_juan(0.8), 3.0),
        (towards_juan(0.8), 3.0),
    ];
    assert_eq!(name_clusters(&[c], &voices), vec![None]);
}

#[test]
fn test_one_name_per_call() {
    let all = vec![(at(0.6), 3.0), (at(0.6), 3.0)];
    let most = vec![
        (at(0.6), 3.0),
        (at(0.6), 3.0),
        (at(0.6), 3.0),
        (basis(5), 3.0),
    ];
    assert_eq!(
        name_clusters(&[most, all], &maria()),
        vec![None, named("María")]
    );
}

#[test]
fn test_remember_merges_by_seconds() {
    let d = tempfile::tempdir().unwrap();
    let mut s = VoiceStore::load(&d.path().join("voces.json"));
    s.remember("María", &basis(0), 10.0);
    s.remember("María", &basis(1), 30.0);
    let v = &s.voices[0];
    assert_eq!(v.seconds, 40.0);
    let want = l2n(&[&[0.25f32][..], &[0.75], &[0.0; DIM - 2]].concat());
    assert!((cosine(&v.print, &want) - 1.0).abs() < 1e-5);
}

#[test]
fn test_store_roundtrip_and_damaged_file() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("voces.json");
    let mut s = VoiceStore::load(&p);
    s.remember("María", &basis(0), 10.0);
    s.save().unwrap();
    assert_eq!(VoiceStore::load(&p).voices, s.voices);
    std::fs::write(&p, "{").unwrap();
    assert!(VoiceStore::load(&p).voices.is_empty());
    assert_eq!(
        std::fs::read_to_string(d.path().join("voces.json.bad")).unwrap(),
        "{"
    );
}

#[test]
fn test_forget_and_rename() {
    let d = tempfile::tempdir().unwrap();
    let mut s = VoiceStore::load(&d.path().join("voces.json"));
    s.remember("María", &basis(0), 10.0);
    s.remember("Juan", &basis(1), 10.0);
    s.rename("Juan", "María").unwrap();
    assert_eq!((s.voices.len(), s.voices[0].seconds), (1, 20.0));
    assert!(s.rename("Nadie", "X").is_err());
    assert!(s.forget("María") && s.voices.is_empty());
}

/// Si `voces.json` existe pero no se pudo leer (p. ej. bloqueado por el antivirus un momento), no
/// se guarda encima: se perderían todas las voces.
#[test]
fn test_unreadable_store_is_not_saved_over() {
    use std::os::windows::fs::OpenOptionsExt;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("voces.json");
    let mut first = VoiceStore::load(&p);
    first.remember("María", &basis(0), 10.0);
    first.save().unwrap();
    let before = std::fs::read(&p).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&p)
        .unwrap();
    let mut s = VoiceStore::load(&p);
    drop(lock);
    s.remember("Juan", &basis(1), 10.0);
    assert!(s.save().is_err());
    assert_eq!(std::fs::read(&p).unwrap(), before);
}

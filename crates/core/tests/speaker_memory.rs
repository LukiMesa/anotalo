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
        (at(0.5), 2.5),
        (at(0.7), 4.0),
        (basis(5), 2.0),
    ];
    assert_eq!(name_clusters(&[c], &maria()), vec![named("María")]);
}

#[test]
fn test_cluster_not_named_below_threshold() {
    let c = vec![(at(0.40), 3.0), (at(0.43), 3.0)];
    assert_eq!(name_clusters(&[c], &maria()), vec![None]);
}

#[test]
fn test_half_or_fewer_votes_is_not_enough() {
    let half = vec![
        (at(0.6), 3.0),
        (at(0.6), 3.0),
        (basis(5), 3.0),
        (basis(6), 3.0),
    ];
    let less = vec![
        (at(0.6), 3.0),
        (at(0.6), 3.0),
        (basis(5), 3.0),
        (basis(6), 3.0),
        (basis(7), 3.0),
    ];
    assert_eq!(
        name_clusters(&[half, less], &maria()),
        vec![named("María"), None]
    );
}

#[test]
fn test_short_segments_vote_only_without_long_ones() {
    let mixed = vec![
        (at(0.6), 3.0),
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

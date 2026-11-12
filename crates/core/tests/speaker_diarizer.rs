//! Las voces de una llamada: etiquetas en vivo, etiquetas finales y huellas de la llamada.
mod common {
    pub mod speaker;
}

use anotalo_core::speaker::diarizer::Diarizer;
use anotalo_core::speaker::memory::StoredVoice;
use common::speaker::{basis, partition, real, voice};

#[test]
fn test_one_voice_stays_others() {
    let mut d = Diarizer::new(vec![], "Otros");
    let live: Vec<String> = (0..10)
        .map(|i| {
            let t = i as f64 * 5.0;
            d.add(t, t + 3.0, Some(voice(0, i))).1
        })
        .collect();
    assert!(live.iter().all(|l| l == "Otros"), "{live:?}");
    let f = d.finish();
    assert!(f.labels.values().all(|l| l == "Otros"));
    assert_eq!(
        (
            f.voices.len(),
            f.voices[0].label.as_str(),
            f.voices[0].seconds
        ),
        (1, "Otros", 30.0)
    );
}

#[test]
fn test_second_voice_switches_live_labels_to_personas() {
    let mut d = Diarizer::new(vec![], "Otros");
    let l: Vec<String> = [0, 0, 1, 0]
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let t = i as f64 * 5.0;
            d.add(t, t + 3.0, Some(voice(*k, i))).1
        })
        .collect();
    assert_eq!(l, ["Otros", "Otros", "Persona 2", "Persona 1"]);
}

#[test]
fn test_finish_numbers_by_first_appearance() {
    let mut d = Diarizer::new(vec![], "Otros");
    for (i, k) in [1, 1, 0, 0, 1, 0].iter().enumerate() {
        let t = i as f64 * 5.0;
        d.add(t, t + 4.0, Some(voice(*k, i)));
    }
    let f = d.finish();
    assert_eq!(
        (f.labels[&0].as_str(), f.labels[&2].as_str()),
        ("Persona 1", "Persona 2")
    );
}

#[test]
fn test_finish_uses_stored_names() {
    let maria = StoredVoice {
        name: "María".into(),
        print: basis(1),
        seconds: 60.0,
    };
    let mut d = Diarizer::new(vec![maria.clone()], "Otros");
    for (i, k) in [0, 1, 0, 1, 0, 1].iter().enumerate() {
        let t = i as f64 * 5.0;
        d.add(t, t + 4.0, Some(voice(*k, i)));
    }
    let f = d.finish();
    assert_eq!(
        (f.labels[&0].as_str(), f.labels[&1].as_str()),
        ("Persona 1", "María")
    );
    // una sola persona (dos segmentos: uno solo no basta para poner nombre)
    let mut one = Diarizer::new(vec![maria.clone()], "Otros");
    one.add(0.0, 4.0, Some(voice(1, 0)));
    one.add(5.0, 9.0, Some(voice(1, 1)));
    assert_eq!(one.finish().labels[&0], "María");
    let mut single = Diarizer::new(vec![maria], "Otros");
    single.add(0.0, 4.0, Some(voice(1, 0)));
    assert_eq!(single.finish().labels[&0], "Otros");
}

#[test]
fn test_segments_without_embedding_follow_the_previous_voice() {
    let mut d = Diarizer::new(vec![], "Otros");
    let ids: Vec<u32> = [None, Some(0), None, Some(1), Some(1), Some(0), None]
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let t = i as f64 * 5.0;
            d.add(t, t + 4.0, k.map(|k| voice(k, i))).0
        })
        .collect();
    let f = d.finish();
    let l = |i: usize| f.labels[&ids[i]].clone();
    assert_eq!((l(0), l(2), l(6)), (l(1), l(1), l(5)));
    assert_ne!(l(1), l(3));
}

#[test]
fn test_live_over_split_is_fixed_at_the_end() {
    let (e, d, _, want) = real("n3_09");
    let mut z = Diarizer::new(vec![], "Otros");
    let mut t = 0.0;
    for (e, d) in e.iter().zip(&d) {
        z.add(t, t + d, Some(e.clone()));
        t += d + 0.5;
    }
    let f = z.finish();
    let got: Vec<String> = (0..want.len() as u32)
        .map(|i| f.labels[&i].clone())
        .collect();
    assert_eq!(partition(&got), partition(&want));
}

#[test]
fn test_no_embeddings_at_all() {
    let mut d = Diarizer::new(vec![], "Otros");
    d.add(0.0, 0.2, None);
    let f = d.finish();
    assert_eq!((f.labels[&0].as_str(), f.voices.len()), ("Otros", 0));
}

/// Una sola persona y 15 s de música en espera (segmentos sin texto): la música no cuenta como
/// persona y el archivo sigue diciendo "Otros".
#[test]
fn test_clusters_without_lines_do_not_count() {
    let mut d = Diarizer::new(vec![], "Otros");
    let mut with_lines = std::collections::HashSet::new();
    for (i, k) in [0, 5, 0, 5, 0, 5, 0].iter().enumerate() {
        let t = i as f64 * 6.0;
        let (id, _) = d.add(t, t + 5.0, Some(voice(*k, i)));
        if *k == 0 {
            with_lines.insert(id);
        }
    }
    assert_ne!(
        d.finish().labels[&0],
        "Otros",
        "sin filtrar, la música es otra persona"
    );
    let f = d.finish_with_lines(&with_lines);
    assert!(with_lines.iter().all(|i| f.labels[i] == "Otros"));
    assert_eq!(f.voices.len(), 1);
}

//! Agrupación de voces en vivo y al colgar, frente al spike (scripts/fixtures/speaker_clusters.py).
mod common {
    pub mod speaker;
}

use anotalo_core::speaker::cluster::{Online, end_of_call};
use anotalo_core::speaker::embed::{DIM, l2n};
use common::speaker::{basis, real, voice};

/// Segmentos sintéticos: (eje, cuántos, segundos).
fn synthetic(groups: &[(usize, usize, f64)]) -> (Vec<Vec<f32>>, Vec<f64>) {
    let (mut e, mut d) = (Vec::new(), Vec::new());
    let mut seed = 0;
    for &(axis, n, secs) in groups {
        for _ in 0..n {
            e.push(voice(axis, seed));
            d.push(secs);
            seed += 1;
        }
    }
    (e, d)
}

/// Vectores unitarios pseudoaleatorios (LCG fijo).
fn random_unit(n: usize, secs: f64) -> (Vec<Vec<f32>>, Vec<f64>) {
    let mut s: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
    };
    let e = (0..n)
        .map(|_| l2n(&(0..DIM).map(|_| next()).collect::<Vec<f32>>()))
        .collect();
    (e, vec![secs; n])
}

#[test]
fn test_online_matches_spike_on_real_calls() {
    for id in ["n3_09", "n4_10"] {
        let (e, d, want, _) = real(id);
        let mut o = Online::new();
        let got: Vec<usize> = e.iter().zip(&d).map(|(e, d)| o.add(e, *d)).collect();
        assert_eq!(got, want, "{id}");
    }
}

#[test]
fn test_end_of_call_matches_spike_on_real_calls() {
    for id in ["n3_09", "n4_10"] {
        let (e, d, _, want) = real(id);
        assert_eq!(end_of_call(&e, &d), want, "{id}");
    }
}

#[test]
fn test_online_short_segment_never_starts_a_speaker() {
    let (a, b) = (basis(0), basis(1));
    let mut o = Online::new();
    assert_eq!(o.add(&a, 3.0), 0);
    assert_eq!(o.add(&b, 0.8), 0); // se une (y acerca el grupo 0 a b)
    assert_eq!(o.add(&basis(2), 1.5), 1);
    assert_eq!(o.len(), 2);
}

#[test]
fn test_end_of_call_merges_clusters_under_10_s() {
    // A: 6×5 s, B: 5×5 s, C: 2×3 s en su propio eje: los segmentos de C van al más cercano de A/B
    let (e, d) = synthetic(&[(0, 6, 5.0), (1, 5, 5.0), (2, 2, 3.0)]);
    let l = end_of_call(&e, &d);
    assert_eq!(l.iter().copied().max(), Some(1), "{l:?}");
}

#[test]
fn test_end_of_call_degenerate_inputs() {
    assert!(end_of_call(&[], &[]).is_empty());
    assert_eq!(end_of_call(&[basis(0)], &[0.5]), vec![0]);
    // un solo segmento ≥ 1 s: se agrupan los dos más largos y el resto los sigue
    let l = end_of_call(&[basis(0), basis(1), basis(0)], &[3.0, 0.9, 0.5]);
    assert_eq!(l, vec![0, 1, 0]);
}

#[test]
fn test_end_of_call_is_fast_for_an_hour() {
    // sin uniones (todo distinto) y con casi todo uniéndose (tres voces): el peor caso
    for (e, d) in [
        random_unit(900, 4.0),
        synthetic(&[(0, 300, 4.0), (1, 300, 4.0), (2, 300, 4.0)]),
    ] {
        let t = std::time::Instant::now();
        let l = end_of_call(&e, &d);
        eprintln!("900 segmentos: {:?}", t.elapsed());
        assert!(t.elapsed().as_secs_f64() < 3.0, "{:?}", t.elapsed());
        assert_eq!(l.len(), 900);
    }
}

#[test]
fn test_end_of_call_is_fast_for_three_hours() {
    // una reunión de 3 h: 3000 segmentos de 3,6 s, cuatro voces; casi todo se une
    let (e, d) = synthetic(&[(0, 750, 3.6), (1, 750, 3.6), (2, 750, 3.6), (3, 750, 3.6)]);
    let t = std::time::Instant::now();
    let l = end_of_call(&e, &d);
    eprintln!("3000 segmentos: {:?}", t.elapsed());
    assert!(t.elapsed().as_secs_f64() < 3.0, "{:?}", t.elapsed());
    assert_eq!(l.len(), 3000);
}

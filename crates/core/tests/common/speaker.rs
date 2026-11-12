//! Ayudas compartidas por los tests de separación de voces.
#![allow(dead_code)]

use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;

use anotalo_core::speaker::embed::{DIM, center, l2n};

pub fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/speaker")
            .join(name),
    )
    .unwrap()
}

pub fn fixture_f32(name: &str) -> Vec<f32> {
    fixture_bytes(name)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

pub fn fixture_u32(name: &str) -> Vec<usize> {
    fixture_bytes(name)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c) as usize)
        .collect()
}

/// Vector unitario del eje `k`.
pub fn basis(k: usize) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[k] = 1.0;
    v
}

/// `basis(k)` con un ruido de ±0,05 en dos ejes altos (distintos por `seed`), normalizado.
pub fn voice(k: usize, seed: usize) -> Vec<f32> {
    let mut v = basis(k);
    let r = |i: usize| ((i * 2_654_435_761) % 1000) as f32 / 1000.0 * 0.1 - 0.05;
    v[100 + seed % 40] += r(seed + 1);
    v[150 + seed % 40] += r(seed + 7);
    l2n(&v)
}

/// Una llamada real del spike: (huellas centradas, duraciones, etiquetas en vivo, etiquetas finales).
pub fn real(id: &str) -> (Vec<Vec<f32>>, Vec<f64>, Vec<usize>, Vec<usize>) {
    let raw = fixture_f32(&format!("{id}.emb.bin"));
    let embs = raw.as_chunks::<DIM>().0.iter().map(|c| center(c)).collect();
    let durs = fixture_f32(&format!("{id}.dur.bin"))
        .into_iter()
        .map(f64::from)
        .collect();
    (
        embs,
        durs,
        fixture_u32(&format!("{id}.online.bin")),
        fixture_u32(&format!("{id}.ahc.bin")),
    )
}

/// Las etiquetas renumeradas por orden de aparición: dos agrupaciones iguales dan lo mismo.
pub fn partition<T: Eq + Hash + Clone>(labels: &[T]) -> Vec<usize> {
    let mut seen = HashMap::new();
    labels
        .iter()
        .map(|l| {
            let n = seen.len();
            *seen.entry(l.clone()).or_insert(n)
        })
        .collect()
}

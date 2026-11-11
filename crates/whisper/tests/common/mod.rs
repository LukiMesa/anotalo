//! Utilidades compartidas por los tests de integración (no es un test).
#![allow(dead_code)]

use std::path::PathBuf;

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

pub fn model() -> PathBuf {
    let p = std::env::var("ANOTALO_TEST_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root().join("target").join("models").join("ggml-tiny.bin"));
    assert!(
        p.exists(),
        "falta el modelo de prueba: ejecuta scripts/get-test-model.ps1 ({})",
        p.display()
    );
    p
}

pub fn jfk() -> Vec<f32> {
    let mut r =
        hound::WavReader::open(root().join("third_party/whisper.cpp/samples/jfk.wav")).unwrap();
    r.samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect()
}

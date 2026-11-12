//! Huellas de voz con TitaNet-small en tract frente a sherpa-onnx (onnxruntime). Necesitan el
//! modelo: `scripts/get-test-model.ps1` (o la variable ANOTALO_TITANET).
use std::path::PathBuf;

use anotalo_core::speaker::embed::{DIM, Embed, Embedder, center, center_checked, cosine, l2n};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn jfk() -> Vec<f32> {
    let mut r =
        hound::WavReader::open(root().join("third_party/whisper.cpp/samples/jfk.wav")).unwrap();
    r.samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect()
}

fn fixture(name: &str) -> Vec<f32> {
    let b = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/speaker")
            .join(name),
    )
    .unwrap();
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn embedder() -> Embedder {
    let path = std::env::var("ANOTALO_TITANET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            root()
                .join("target")
                .join("models")
                .join("nemo_en_titanet_small.onnx")
        });
    assert!(
        path.exists(),
        "falta {}: ejecuta scripts/get-test-model.ps1",
        path.display()
    );
    Embedder::load(&path).unwrap()
}

fn cos64(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum();
    let n = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    dot / (n(a) * n(b))
}

#[test]
fn test_matches_sherpa_on_jfk() {
    let e = embedder();
    let x = jfk();
    let t = std::time::Instant::now();
    let full = e.embed_raw(&x).unwrap();
    eprintln!("11 s de audio: {:?}", t.elapsed());
    assert!(cos64(&full, &fixture("jfk_full.emb.bin")) > 0.9999);
    let three = e.embed_raw(&x[0..48000]).unwrap();
    assert!(cos64(&three, &fixture("jfk_3s.emb.bin")) > 0.9999);
}

#[test]
fn test_short_segment_embeds() {
    let e = embedder();
    let x = jfk();
    let v = e.embed(&x[16000..20800]).expect("0,3 s se embebe");
    assert_eq!(v.len(), DIM);
    assert!((v.iter().map(|a| a * a).sum::<f32>().sqrt() - 1.0).abs() < 1e-4);
    assert!(e.embed(&x[16000..20640]).is_none(), "0,29 s no");
}

#[test]
fn test_center_uses_the_shipped_mean() {
    let raw = fixture("jfk_full.emb.bin");
    let c = center(&raw);
    assert!((c.iter().map(|a| a * a).sum::<f32>().sqrt() - 1.0).abs() < 1e-5);
    assert!(
        cosine(&c, &l2n(&raw)) < 0.999,
        "el centrado cambia el vector"
    );
}

/// Un NaN se guardaría como `null` y dejaría ilegible el archivo de voces de la llamada.
#[test]
fn test_non_finite_print_is_rejected() {
    let mut raw = fixture("jfk_full.emb.bin");
    assert_eq!(center_checked(&raw).unwrap(), center(&raw));
    raw[7] = f32::NAN;
    assert!(center_checked(&raw).is_err(), "NaN");
    raw[7] = f32::INFINITY;
    assert!(center_checked(&raw).is_err(), "infinito");
}

#[test]
fn test_same_speaker_matches_across_halves() {
    let e = embedder();
    let x = jfk();
    let (a, b) = (
        e.embed(&x[0..80000]).unwrap(),
        e.embed(&x[88000..176000]).unwrap(),
    );
    assert!(cosine(&a, &b) >= 0.44, "{}", cosine(&a, &b));
}

#[test]
fn test_load_rejects_a_bad_file() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x.onnx");
    std::fs::write(&p, b"no es un modelo").unwrap();
    assert!(Embedder::load(&p).is_err());
}

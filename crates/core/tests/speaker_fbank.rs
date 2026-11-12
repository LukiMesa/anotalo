//! Las características de TitaNet frente a kaldi-native-fbank con las opciones de sherpa-onnx
//! (fixtures de scripts/fixtures/speaker_reference.py).
use std::path::PathBuf;

use anotalo_core::speaker::fbank::{N_MELS, fbank, normalize_per_feature, num_frames};

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

fn fixture(name: &str) -> Vec<[f32; N_MELS]> {
    let b = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/speaker")
            .join(name),
    )
    .unwrap();
    let v: Vec<f32> = b
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    v.as_chunks::<N_MELS>().0.to_vec()
}

fn max_abs_diff(a: &[[f32; N_MELS]], b: &[[f32; N_MELS]]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[test]
fn test_frame_count() {
    assert_eq!([399, 400, 9600, 48000].map(num_frames), [0, 1, 58, 298]);
}

#[test]
fn test_matches_kaldi_on_jfk_3s() {
    let got = fbank(&jfk()[0..48000]);
    let want = fixture("jfk_3s.fbank.bin");
    assert_eq!(got.len(), 298);
    let d = max_abs_diff(&got, &want);
    assert!(d < 2e-3, "{d}");
}

#[test]
fn test_matches_kaldi_on_short_segment() {
    let got = fbank(&jfk()[16000..25600]);
    assert_eq!(got.len(), 58);
    let d = max_abs_diff(&got, &fixture("jfk_short.fbank.bin"));
    assert!(d < 2e-3, "{d}");
}

#[test]
fn test_silence_is_finite() {
    let f = fbank(&[0.0; 16000]);
    assert!(f.iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn test_normalize_per_feature() {
    let mut f = fbank(&jfk()[0..48000]);
    normalize_per_feature(&mut f);
    for b in 0..N_MELS {
        let col: Vec<f64> = f.iter().map(|r| f64::from(r[b])).collect();
        let mean = col.iter().sum::<f64>() / col.len() as f64;
        let std = (col.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / col.len() as f64).sqrt();
        assert!(
            mean.abs() < 1e-4 && (std - 1.0).abs() < 1e-3,
            "bin {b}: {mean} {std}"
        );
    }
    let mut flat = vec![[1.5f32; N_MELS]; 10];
    normalize_per_feature(&mut flat);
    assert!(flat.iter().flatten().all(|v| *v == 0.0));
}

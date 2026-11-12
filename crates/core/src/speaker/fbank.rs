//! Las características que TitaNet espera (spec §2.4.5): el fbank de 80 bandas de kaldi-native-fbank
//! con las opciones que sherpa-onnx usa para los modelos de NeMo. Muestras de 16 kHz en [-1, 1],
//! tramas de 25 ms cada 10 ms sin rellenar los bordes, sin dither ni quitar la continua,
//! preénfasis 0,97, ventana de Hann periódica, FFT de 512, espectro de potencia, banco mel de
//! Slaney (0–7600 Hz, normalizado por área) y logaritmo con suelo `f32::EPSILON`.

use std::sync::{Arc, OnceLock};

use realfft::{RealFftPlanner, RealToComplex};

pub const N_MELS: usize = 80;
const SR: f64 = 16_000.0;
const FRAME: usize = 400;
const SHIFT: usize = 160;
const N_FFT: usize = 512;
const N_BINS: usize = N_FFT / 2 + 1;
const PREEMPH: f32 = 0.97;
const F_MAX: f64 = 7_600.0;

/// Tramas de `n` muestras (sin rellenar los bordes).
pub fn num_frames(n_samples: usize) -> usize {
    if n_samples < FRAME {
        0
    } else {
        1 + (n_samples - FRAME) / SHIFT
    }
}

struct Tables {
    window: Vec<f32>,
    /// Banco mel `[N_MELS][N_BINS]`.
    mel: Vec<[f32; N_BINS]>,
    fft: Arc<dyn RealToComplex<f32>>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let window = (0..FRAME)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FRAME as f64).cos()) as f32)
            .collect();
        Tables {
            window,
            mel: slaney_mel(),
            fft: RealFftPlanner::<f32>::new().plan_fft_forward(N_FFT),
        }
    })
}

/// Escala mel de Slaney (la de librosa por defecto).
fn hz_to_mel(f: f64) -> f64 {
    if f >= 1000.0 {
        15.0 + (f / 1000.0).ln() / (6.4f64.ln() / 27.0)
    } else {
        f / (200.0 / 3.0)
    }
}

fn mel_to_hz(m: f64) -> f64 {
    if m >= 15.0 {
        1000.0 * ((6.4f64.ln() / 27.0) * (m - 15.0)).exp()
    } else {
        m * (200.0 / 3.0)
    }
}

/// `librosa.filters.mel(sr=16000, n_fft=512, n_mels=80, fmin=0, fmax=7600)` (norm="slaney").
fn slaney_mel() -> Vec<[f32; N_BINS]> {
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(F_MAX));
    let pts: Vec<f64> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (N_MELS + 1) as f64))
        .collect();
    let freqs: Vec<f64> = (0..N_BINS)
        .map(|k| SR / 2.0 * k as f64 / (N_BINS - 1) as f64)
        .collect();
    (0..N_MELS)
        .map(|m| {
            let (f0, f1, f2) = (pts[m], pts[m + 1], pts[m + 2]);
            let norm = 2.0 / (f2 - f0);
            let mut row = [0f32; N_BINS];
            for (k, w) in row.iter_mut().enumerate() {
                let lower = (freqs[k] - f0) / (f1 - f0);
                let upper = (f2 - freqs[k]) / (f2 - f1);
                *w = (lower.min(upper).max(0.0) * norm) as f32;
            }
            row
        })
        .collect()
}

/// Log-energías mel por trama, `[tramas][80]`.
pub fn fbank(samples: &[f32]) -> Vec<[f32; N_MELS]> {
    let t = tables();
    let mut input = t.fft.make_input_vec();
    let mut spectrum = t.fft.make_output_vec();
    let mut power = [0f32; N_BINS];
    (0..num_frames(samples.len()))
        .map(|i| {
            let frame = &samples[i * SHIFT..i * SHIFT + FRAME];
            input.fill(0.0);
            input[..FRAME].copy_from_slice(frame);
            for j in (1..FRAME).rev() {
                input[j] -= PREEMPH * input[j - 1];
            }
            input[0] -= PREEMPH * input[0];
            for (x, w) in input[..FRAME].iter_mut().zip(&t.window) {
                *x *= w;
            }
            t.fft
                .process(&mut input, &mut spectrum)
                .expect("tamaños de la FFT");
            for (p, c) in power.iter_mut().zip(&spectrum) {
                *p = c.norm_sqr();
            }
            let mut out = [0f32; N_MELS];
            for (o, row) in out.iter_mut().zip(&t.mel) {
                let e: f32 = row.iter().zip(&power).map(|(w, p)| w * p).sum();
                *o = e.max(f32::EPSILON).ln();
            }
            out
        })
        .collect()
}

/// Normalización por banda sobre el segmento: media 0 y desviación (poblacional) 1, con 1e-5 en el
/// divisor como sherpa-onnx.
pub fn normalize_per_feature(feats: &mut [[f32; N_MELS]]) {
    if feats.is_empty() {
        return;
    }
    let n = feats.len() as f64;
    for b in 0..N_MELS {
        let mean = feats.iter().map(|r| f64::from(r[b])).sum::<f64>() / n;
        let var = feats
            .iter()
            .map(|r| (f64::from(r[b]) - mean).powi(2))
            .sum::<f64>()
            / n;
        let div = var.sqrt() + 1e-5;
        for r in feats.iter_mut() {
            r[b] = ((f64::from(r[b]) - mean) / div) as f32;
        }
    }
}

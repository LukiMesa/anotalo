//! Señales sintéticas para los tests de la cancelación de eco: voces, el eco de los altavoces y
//! ruido, deterministas y a 16 kHz.
#![allow(dead_code)]

use std::f32::consts::TAU;

pub const SR: usize = 16_000;

/// Números al azar deterministas (xorshift).
pub struct Rng(pub u64);

impl Rng {
    /// Uniforme en [0, 1).
    pub fn unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniforme en [-1, 1).
    pub fn sym(&mut self) -> f32 {
        (2.0 * self.unit() - 1.0) as f32
    }
}

pub fn samples(secs: f64) -> usize {
    (secs * SR as f64).round() as usize
}

pub fn energy(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum()
}

pub fn rms(x: &[f32]) -> f32 {
    (energy(x) / x.len().max(1) as f64).sqrt() as f32
}

/// Algo parecido a una voz: armónicos de un tono que sube y baja, con formantes, sílabas de unos
/// 4 Hz, un poco de soplo y pausas entre palabras; 0,1 de nivel (RMS) mientras suena. `seed` y
/// `f0` distinguen a las personas.
pub fn voice(seed: u64, secs: f64, f0: f32) -> Vec<f32> {
    let n = samples(secs);
    let mut rng = Rng(seed);
    // palabras: 0,4–1,2 s de voz y 0,15–0,5 s de silencio
    let mut gate = vec![0.0f32; n];
    let mut i = samples(0.1);
    while i < n {
        let len = samples(0.4 + 0.8 * rng.unit());
        for k in 0..len.min(n - i) {
            gate[i + k] = (k.min(len - k) as f32 / 320.0).min(1.0);
        }
        i += len + samples(0.15 + 0.35 * rng.unit());
    }
    let rate = 3.5 + rng.unit() as f32;
    let mut x = vec![0.0f32; n];
    let mut phase = 0.0f32;
    let mut breath = 0.0f32;
    let mut amps: Vec<f32> = Vec::new();
    let mut pitch = f0;
    for (k, v) in x.iter_mut().enumerate() {
        let t = k as f32 / SR as f32;
        if k % 160 == 0 {
            // el tono y sus formantes cambian despacio: se recalculan cada 10 ms
            pitch = f0 * (1.0 + 0.12 * (TAU * 0.7 * t).sin() + 0.05 * (TAU * 2.1 * t + 1.0).sin());
            amps = (1..)
                .map(|h| h as f32 * pitch)
                .take_while(|f| *f < 3800.0)
                .enumerate()
                .map(|(h, f)| {
                    let peak = |c: f32, w: f32| (-((f - c) / w).powi(2)).exp();
                    (1.0 + 3.0 * peak(700.0, 250.0)
                        + 2.0 * peak(1800.0, 350.0)
                        + peak(2800.0, 400.0))
                        / (h + 1) as f32
                })
                .collect();
        }
        phase = (phase + TAU * pitch / SR as f32) % TAU;
        let s: f32 = amps
            .iter()
            .enumerate()
            .map(|(h, a)| a * ((h + 1) as f32 * phase).sin())
            .sum();
        breath = 0.7 * breath + 0.3 * rng.sym();
        let syllable = 0.55 - 0.45 * (TAU * rate * t).cos();
        *v = gate[k] * syllable * (s + 0.3 * breath);
    }
    let voiced: Vec<f32> = x.iter().copied().filter(|v| *v != 0.0).collect();
    let g = 0.1 / rms(&voiced);
    x.iter_mut().for_each(|v| *v *= g);
    x
}

/// Lo que el micrófono oye de los altavoces: `far` con `delay_s` de retardo, por una sala (sonido
/// directo y reflexiones que se apagan en unos 25 ms), atenuado a `gain`.
pub fn echo(far: &[f32], delay_s: f64, gain: f32) -> Vec<f32> {
    let mut rng = Rng(11);
    let h: Vec<f32> = (0..400)
        .map(|k| match k {
            0 => 1.0,
            _ => 0.6 * rng.sym() * (-(k as f32) / 100.0).exp(),
        })
        .collect();
    let norm = h.iter().map(|v| v * v).sum::<f32>().sqrt();
    let d = samples(delay_s);
    let mut y = vec![0.0f32; far.len()];
    for (m, v) in y.iter_mut().skip(d).enumerate() {
        let acc: f32 = h
            .iter()
            .take(m + 1)
            .enumerate()
            .map(|(k, hk)| hk * far[m - k])
            .sum();
        *v = gain * acc / norm;
    }
    y
}

/// Ruido de fondo del micrófono (unos -70 dBFS).
pub fn noise(seed: u64, n: usize) -> Vec<f32> {
    let mut rng = Rng(seed);
    (0..n).map(|_| 0.0005 * rng.sym()).collect()
}

pub fn mix(parts: &[&[f32]]) -> Vec<f32> {
    let n = parts.iter().map(|p| p.len()).max().unwrap_or(0);
    let mut y = vec![0.0f32; n];
    for p in parts {
        y.iter_mut().zip(p.iter()).for_each(|(a, b)| *a += b);
    }
    y
}

/// `x` con silencio salvo en [from, to) segundos.
pub fn only(x: &[f32], from: f64, to: f64) -> Vec<f32> {
    let (a, b) = (samples(from), samples(to).min(x.len()));
    let mut y = vec![0.0f32; x.len()];
    y[a..b].copy_from_slice(&x[a..b]);
    y
}

//! Cualquier formato de captura a float32 mono de 16 kHz, en streaming (port de calltx/audio/resample.py).

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _};

pub const TARGET_RATE: u32 = 16_000;
/// Tramos de entrada del remuestreador FFT (≈21 ms a 48 kHz).
const CHUNK: usize = 1024;

/// Convierte audio entrelazado (int16 o float) a mono de 16 kHz, guardando estado entre llamadas.
/// Como en Python: int16 se escala por 1/32768, los canales se promedian (se descarta un marco
/// incompleto al final de cada bloque) y a 16 kHz pasa sin filtrar.
pub struct Resampler {
    channels: usize,
    in_rate: u32,
    fft: Option<Fft<f32>>, // None: ya es de 16 kHz
    pending: Vec<f32>,     // mono a la frecuencia de entrada, esperando un tramo completo
    out_buf: Vec<f32>,
    to_skip: usize, // retardo inicial del filtro que aún hay que descartar
    in_frames: u64,
    out_frames: u64,
}

impl Resampler {
    pub fn new(in_rate: u32, channels: u16) -> Resampler {
        let fft = (in_rate != TARGET_RATE).then(|| {
            Fft::<f32>::new(
                in_rate as usize,
                TARGET_RATE as usize,
                CHUNK,
                1,
                FixedSync::Input,
            )
            .expect("frecuencia de entrada no válida para el remuestreador")
        });
        let (out_buf, to_skip) = match &fft {
            Some(f) => (vec![0.0; f.output_frames_max()], f.output_delay()),
            None => (Vec::new(), 0),
        };
        Resampler {
            channels: channels.max(1) as usize,
            in_rate,
            fft,
            pending: Vec::new(),
            out_buf,
            to_skip,
            in_frames: 0,
            out_frames: 0,
        }
    }

    pub fn push_i16(&mut self, interleaved: &[i16]) -> Vec<f32> {
        let x: Vec<f32> = interleaved
            .iter()
            .map(|&s| f32::from(s) / 32768.0)
            .collect();
        self.push_f32(&x)
    }

    pub fn push_f32(&mut self, interleaved: &[f32]) -> Vec<f32> {
        let mono: Vec<f32> = interleaved
            .chunks_exact(self.channels)
            .map(|frame| frame.iter().sum::<f32>() / self.channels as f32)
            .collect();
        self.in_frames += mono.len() as u64;
        if self.fft.is_none() {
            self.out_frames += mono.len() as u64;
            return mono;
        }
        self.pending.extend_from_slice(&mono);
        let mut out = Vec::new();
        loop {
            let need = self
                .fft
                .as_ref()
                .map_or(usize::MAX, |f| f.input_frames_next());
            if self.pending.len() < need {
                break;
            }
            let chunk: Vec<f32> = self.pending.drain(..need).collect();
            out.extend(self.run(&chunk, None));
        }
        out
    }

    /// Fin del flujo: devuelve lo que quedaba (con el retardo del filtro vaciado) hasta completar
    /// exactamente la duración de lo recibido.
    pub fn finish(&mut self) -> Vec<f32> {
        if self.fft.is_none() {
            return Vec::new();
        }
        let expected =
            (self.in_frames as f64 * TARGET_RATE as f64 / self.in_rate as f64).round() as u64;
        let mut out = Vec::new();
        let rest = std::mem::take(&mut self.pending);
        if !rest.is_empty() {
            out.extend(self.run(&rest, Some(rest.len())));
        }
        while self.out_frames + (out.len() as u64) < expected {
            let before = out.len();
            out.extend(self.run(&[], Some(0)));
            if out.len() == before {
                break;
            }
        }
        out.truncate(expected.saturating_sub(self.out_frames) as usize);
        self.out_frames += out.len() as u64;
        out
    }

    fn run(&mut self, input: &[f32], partial: Option<usize>) -> Vec<f32> {
        let fft = self.fft.as_mut().expect("solo con remuestreo");
        let n_in = fft.input_frames_next();
        let mut inbuf = input.to_vec();
        inbuf.resize(n_in, 0.0);
        let adapter_in = InterleavedSlice::new(&inbuf, 1, n_in).expect("tramo de entrada");
        let cap = self.out_buf.len();
        let mut adapter_out =
            InterleavedSlice::new_mut(&mut self.out_buf, 1, cap).expect("tramo de salida");
        let mut indexing = Indexing::new();
        indexing.partial_len = partial;
        let (_, n_out) = fft
            .process_into_buffer(&adapter_in, &mut adapter_out, Some(&indexing))
            .expect("el remuestreador falló");
        let skip = self.to_skip.min(n_out);
        self.to_skip -= skip;
        let produced = self.out_buf[skip..n_out].to_vec();
        if partial.is_none() {
            self.out_frames += produced.len() as u64;
        }
        produced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_stereo_48k(seconds: f64, freq: f64) -> Vec<f32> {
        let n = (48_000.0 * seconds) as usize;
        let mut out = Vec::with_capacity(n * 2);
        for i in 0..n {
            let v = (2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin() as f32 * 0.5;
            out.push(v);
            out.push(v);
        }
        out
    }

    #[test]
    fn test_resample_48k_stereo_to_16k_mono_length() {
        let mut r = Resampler::new(48_000, 2);
        let mut out = r.push_f32(&vec![0.0; 48_000 * 2]);
        out.extend(r.finish());
        assert!((out.len() as i64 - 16_000).abs() <= 1, "{}", out.len());
    }

    #[test]
    fn test_resample_int16_scaled() {
        let mut r = Resampler::new(16_000, 1);
        let out = r.push_i16(&vec![32767; 1600]);
        let max = out.iter().cloned().fold(f32::MIN, f32::max);
        assert!(max > 0.99 && max <= 1.0, "{max}");
    }

    #[test]
    fn test_stereo_averaged() {
        let mut r = Resampler::new(16_000, 2);
        let x: Vec<f32> = (0..1600)
            .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
            .collect();
        let out = r.push_f32(&x);
        assert_eq!(out.len(), 800);
        assert!(out.iter().all(|v| (v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn streaming_matches_one_shot() {
        let x = sine_stereo_48k(3.0, 1000.0);
        let mut one = Resampler::new(48_000, 2);
        let mut a = one.push_f32(&x);
        a.extend(one.finish());
        let mut chunked = Resampler::new(48_000, 2);
        let mut b = Vec::new();
        for chunk in x.chunks(960 * 2) {
            b.extend(chunked.push_f32(chunk)); // 10 ms
        }
        b.extend(chunked.finish());
        for v in [&a, &b] {
            assert!((v.len() as f64 - 48_000.0).abs() <= 480.0, "{}", v.len());
        }
        let skip = 320; // 20 ms a 16 kHz
        let n = a.len().min(b.len());
        let worst = (skip..n)
            .map(|i| (a[i] - b[i]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "{worst}");
        let crossings = b[skip..n]
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        let freq = crossings as f64 / 2.0 / ((n - skip) as f64 / 16_000.0);
        assert!((freq - 1000.0).abs() <= 5.0, "{freq}");
    }
}

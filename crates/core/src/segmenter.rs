//! Corta un flujo de audio en frases con un detector de voz por tramas (port de calltx/segmenter.py).
//!
//! Una diferencia deliberada con Python: tras un corte forzado (`max_segment_s`), la frase
//! siguiente empieza donde terminó la anterior antes de recortar su silencio final. Python usaba
//! la longitud ya recortada y adelantaba ese inicio (spike de separación de voces, spec §2.4.5).

use std::collections::VecDeque;

pub const SR: f64 = 16_000.0;
pub const FRAME: usize = 512;
pub const FRAME_S: f64 = FRAME as f64 / SR;
/// Retrasos de callback por debajo de esto son jitter, no huecos reales.
const GAP_TOLERANCE_S: f64 = 0.5;
const TRAILING_KEEP_S: f64 = 0.2;

/// Detector de voz por trama: probabilidad de voz de 512 muestras.
pub type VadFn = Box<dyn FnMut(&[f32]) -> f32 + Send>;

/// Una frase: quién habla, cuándo (segundos desde el inicio de la grabación) y su audio.
#[derive(Debug, Clone)]
pub struct Segment {
    pub speaker: String,
    pub start_s: f64,
    pub end_s: f64,
    pub audio: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    pub silence_ms: u32,
    pub max_segment_s: f64,
    pub min_segment_ms: u32,
    pub threshold: f32,
    pub pre_roll_ms: u32,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        SegmenterConfig {
            silence_ms: 600,
            max_segment_s: 25.0,
            min_segment_ms: 300,
            threshold: 0.5,
            pre_roll_ms: 200,
        }
    }
}

pub struct Segmenter {
    speaker: String,
    vad: VadFn,
    silence_s: f64,
    max_s: f64,
    min_voiced_s: f64,
    on: f32,
    off: f32,
    preroll: VecDeque<(f64, Vec<f32>)>,
    preroll_cap: usize,
    buf: Vec<f32>,
    buf_t: Option<f64>,
    frames: Vec<Vec<f32>>,
    seg_start: f64,
    voiced_s: f64,
    silence_run: f64,
    /// Tramas silenciosas al final de la frase (sin contar los huecos, que no tienen tramas).
    silent_tail: usize,
    in_speech: bool,
    out: Vec<Segment>,
}

impl Segmenter {
    pub fn new(speaker: &str, cfg: SegmenterConfig, vad: VadFn) -> Segmenter {
        let preroll_cap = ((cfg.pre_roll_ms as f64 / 1000.0 / FRAME_S).round() as usize).max(1);
        Segmenter {
            speaker: speaker.to_string(),
            vad,
            silence_s: cfg.silence_ms as f64 / 1000.0,
            max_s: cfg.max_segment_s,
            min_voiced_s: cfg.min_segment_ms as f64 / 1000.0,
            on: cfg.threshold,
            off: cfg.threshold - 0.15,
            preroll: VecDeque::with_capacity(preroll_cap),
            preroll_cap,
            buf: Vec::new(),
            buf_t: None,
            frames: Vec::new(),
            seg_start: 0.0,
            voiced_s: 0.0,
            silence_run: 0.0,
            silent_tail: 0,
            in_speech: false,
            out: Vec::new(),
        }
    }

    fn reset_segment(&mut self) {
        self.frames = Vec::new();
        self.seg_start = 0.0;
        self.voiced_s = 0.0;
        self.silence_run = 0.0;
        self.silent_tail = 0;
        self.in_speech = false;
    }

    // --- entrada -----------------------------------------------------------------------

    /// Audio mono de 16 kHz que empieza en `t_s`. Devuelve las frases que se cerraron.
    pub fn feed(&mut self, t_s: f64, samples: &[f32]) -> Vec<Segment> {
        if samples.is_empty() {
            return Vec::new();
        }
        match self.buf_t {
            None => self.buf_t = Some(t_s),
            Some(buf_t) => {
                let expected = buf_t + self.buf.len() as f64 / SR;
                let mut gap = t_s - expected;
                if gap > GAP_TOLERANCE_S {
                    // Procesa lo que quedaba en el buffer (relleno con ceros) antes del hueco.
                    if !self.buf.is_empty() {
                        let pad = FRAME - self.buf.len();
                        let mut frame = std::mem::take(&mut self.buf);
                        frame.resize(FRAME, 0.0);
                        self.process(buf_t, frame);
                        gap -= pad as f64 / SR;
                    }
                    self.on_gap(gap.max(0.0));
                    self.buf.clear();
                    self.buf_t = Some(t_s);
                } else if gap < -GAP_TOLERANCE_S {
                    // El reloj del dispositivo va por delante: realinea el tiempo sin tirar audio.
                    self.buf_t = Some(t_s - self.buf.len() as f64 / SR);
                }
            }
        }
        self.buf.extend_from_slice(samples);
        let n = self.buf.len() / FRAME;
        let start = self.buf_t.unwrap_or(t_s);
        for i in 0..n {
            let frame = self.buf[i * FRAME..(i + 1) * FRAME].to_vec();
            self.process(start + i as f64 * FRAME_S, frame);
        }
        if n > 0 {
            self.buf.drain(..n * FRAME);
            self.buf_t = Some(start + n as f64 * FRAME_S);
        }
        std::mem::take(&mut self.out)
    }

    /// Marca de reloj sin audio: si la fuente dejó de mandar paquetes, cierra la frase abierta.
    pub fn idle(&mut self, t_s: f64) -> Vec<Segment> {
        if let (true, Some(buf_t)) = (self.in_speech, self.buf_t) {
            let gap = t_s - (buf_t + self.buf.len() as f64 / SR);
            if gap > 0.0 && self.silence_run + gap >= self.silence_s {
                self.finish(false);
            }
        }
        std::mem::take(&mut self.out)
    }

    pub fn flush(&mut self) -> Vec<Segment> {
        if self.in_speech {
            self.finish(false);
        }
        self.preroll.clear();
        std::mem::take(&mut self.out)
    }

    // --- lógica ------------------------------------------------------------------------

    /// La captura de sistema no entrega paquetes en silencio: el hueco cuenta como silencio.
    fn on_gap(&mut self, gap: f64) {
        self.preroll.clear();
        if !self.in_speech {
            return;
        }
        self.silence_run += gap;
        if self.silence_run >= self.silence_s {
            self.finish(false);
        }
    }

    fn process(&mut self, t: f64, frame: Vec<f32>) {
        let p = (self.vad)(&frame);
        if !self.in_speech {
            if p >= self.on {
                self.in_speech = true;
                let pre: Vec<(f64, Vec<f32>)> = self.preroll.drain(..).collect();
                self.seg_start = pre.first().map_or(t, |(pt, _)| *pt);
                self.frames = pre.into_iter().map(|(_, f)| f).collect();
                self.frames.push(frame);
                self.voiced_s = FRAME_S;
                self.silence_run = 0.0;
                self.silent_tail = 0;
            } else {
                if self.preroll.len() == self.preroll_cap {
                    self.preroll.pop_front();
                }
                self.preroll.push_back((t, frame));
            }
            return;
        }
        self.frames.push(frame);
        if p < self.off {
            self.silence_run += FRAME_S;
            self.silent_tail += 1;
        } else {
            self.silence_run = 0.0;
            self.silent_tail = 0;
            self.voiced_s += FRAME_S;
        }
        if self.silence_run >= self.silence_s {
            self.finish(false);
        } else if self.frames.len() as f64 * FRAME_S >= self.max_s {
            self.finish(true);
        }
    }

    fn finish(&mut self, keep_speaking: bool) {
        let mut frames = std::mem::take(&mut self.frames);
        let total = frames.len();
        // Recorta el silencio final dejando un pequeño margen. Solo tramas silenciosas de verdad:
        // el tiempo de un hueco sin paquetes no tiene tramas (Python recortaba voz aquí).
        let silent_frames = total.min(self.silent_tail);
        let keep = (TRAILING_KEEP_S / FRAME_S).round() as usize;
        let drop = silent_frames.saturating_sub(keep);
        frames.truncate(total - drop);
        let start = self.seg_start;
        let voiced = self.voiced_s;
        self.reset_segment();
        if keep_speaking {
            self.in_speech = true;
            // Antes de recortar: la siguiente frase empieza justo tras la última trama recibida.
            self.seg_start = start + total as f64 * FRAME_S;
        }
        if frames.is_empty() || voiced < self.min_voiced_s {
            return;
        }
        let audio: Vec<f32> = frames.concat();
        let end = start + audio.len() as f64 / SR;
        self.out.push(Segment {
            speaker: self.speaker.clone(),
            start_s: start,
            end_s: end,
            audio,
        });
    }
}

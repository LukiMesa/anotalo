//! Port de tests/test_segmenter.py (mismo VAD falso y mismas entradas) más el arreglo del corte forzado.
use anotalo_core::segmenter::{Segment, Segmenter, SegmenterConfig, VadFn};

const SR: f64 = 16_000.0;

fn fake_vad() -> VadFn {
    Box::new(|frame: &[f32]| {
        let rms = (frame.iter().map(|v| v * v).sum::<f32>() / frame.len() as f32).sqrt();
        if rms > 0.01 { 1.0 } else { 0.0 }
    })
}

fn tone(seconds: f64) -> Vec<f32> {
    (0..(seconds * SR) as usize)
        .map(|i| (0.3 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / SR).sin()) as f32)
        .collect()
}

fn silence(seconds: f64) -> Vec<f32> {
    vec![0.0; (seconds * SR) as usize]
}

fn cat(parts: &[Vec<f32>]) -> Vec<f32> {
    parts.concat()
}

fn segmenter(speaker: &str, cfg: SegmenterConfig) -> Segmenter {
    Segmenter::new(speaker, cfg, fake_vad())
}

/// Alimenta en trozos de 10 ms, como un callback de captura real.
fn run(chunks: &[(f64, Vec<f32>)], cfg: SegmenterConfig) -> Vec<Segment> {
    let mut seg = segmenter("Yo", cfg);
    let mut out = Vec::new();
    for (t, x) in chunks {
        for (k, piece) in x.chunks(160).enumerate() {
            out.extend(seg.feed(t + (k * 160) as f64 / SR, piece));
        }
    }
    out.extend(seg.flush());
    out
}

fn feed_with_jitter(seg: &mut Segmenter, x: &[f32], out: &mut Vec<Segment>) {
    for (n, piece) in x.chunks(800).enumerate() {
        let jitter = if n % 2 == 1 { 0.08 } else { 0.0 };
        out.extend(seg.feed((n * 800) as f64 / SR + jitter, piece));
    }
}

#[test]
fn single_utterance() {
    let x = cat(&[silence(0.5), tone(1.0), silence(1.0)]);
    let segs = run(&[(0.0, x)], SegmenterConfig::default());
    assert_eq!(segs.len(), 1);
    let s = &segs[0];
    assert_eq!(s.speaker, "Yo");
    assert!((s.start_s - 0.5).abs() < 0.25, "{}", s.start_s);
    assert!((s.end_s - 1.5).abs() < 0.7, "{}", s.end_s);
    assert!((s.audio.len() as f64 - (s.end_s - s.start_s) * SR).abs() <= SR * 0.05);
}

#[test]
fn short_blip_dropped() {
    let x = cat(&[silence(0.5), tone(0.1), silence(1.0)]);
    assert!(run(&[(0.0, x)], SegmenterConfig::default()).is_empty());
}

#[test]
fn force_cut() {
    let segs = run(
        &[(0.0, tone(30.0))],
        SegmenterConfig {
            max_segment_s: 25.0,
            ..Default::default()
        },
    );
    assert_eq!(segs.len(), 2);
    assert!(segs[0].end_s - segs[0].start_s <= 25.0 + 0.05);
}

#[test]
fn gap_counts_as_silence() {
    let segs = run(
        &[(0.0, tone(1.0)), (5.0, tone(1.0))],
        SegmenterConfig::default(),
    );
    assert_eq!(segs.len(), 2);
    assert!((segs[1].start_s - 5.0).abs() < 0.25, "{}", segs[1].start_s);
    assert!(segs[0].end_s < 2.0);
}

#[test]
fn flush_emits_open() {
    let mut seg = segmenter("Otros", SegmenterConfig::default());
    let mut out = Vec::new();
    for (k, piece) in tone(1.0).chunks(160).enumerate() {
        out.extend(seg.feed((k * 160) as f64 / SR, piece));
    }
    assert!(out.is_empty());
    out.extend(seg.flush());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].speaker, "Otros");
}

#[test]
fn silero_loads() {
    let mut v = anotalo_core::vad::Vad::new().unwrap();
    assert!(v.prob(&[0.0; 512]) < 0.5);
}

#[test]
fn callback_jitter_does_not_split_or_shift() {
    // Callbacks que llegan 80 ms tarde no son huecos: una sola frase y tiempos correctos.
    let mut seg = segmenter("Yo", SegmenterConfig::default());
    let mut out = Vec::new();
    feed_with_jitter(
        &mut seg,
        &cat(&[silence(0.5), tone(3.0), silence(1.0)]),
        &mut out,
    );
    out.extend(seg.flush());
    assert_eq!(out.len(), 1);
    assert!((out[0].start_s - 0.5).abs() < 0.25);
    assert!((out[0].end_s - 3.5).abs() < 0.5);
    // 3 s de voz + ~0.2 s de pre-roll + ~0.2 s de margen final
    assert!(
        (out[0].audio.len() as f64 / SR - 3.4).abs() < 0.1,
        "{}",
        out[0].audio.len()
    );
}

#[test]
fn idle_finishes_open_segment() {
    let mut seg = segmenter("Otros", SegmenterConfig::default());
    let mut out = Vec::new();
    for (k, piece) in tone(1.0).chunks(160).enumerate() {
        out.extend(seg.feed((k * 160) as f64 / SR, piece));
    }
    out.extend(seg.idle(3.0));
    assert_eq!(out.len(), 1);
}

#[test]
fn jitter_does_not_ratchet_later_timestamps() {
    let mut seg = segmenter("Yo", SegmenterConfig::default());
    let mut out = Vec::new();
    let x = cat(&[
        silence(0.5),
        tone(3.0),
        silence(2.5),
        tone(1.0),
        silence(1.0),
    ]);
    feed_with_jitter(&mut seg, &x, &mut out);
    out.extend(seg.flush());
    assert_eq!(out.len(), 2);
    assert!((out[1].start_s - 6.0).abs() < 0.3, "{}", out[1].start_s);
}

#[test]
fn force_cut_in_a_short_pause_keeps_the_next_start_exact() {
    // El corte de 25 s cae 0,42 s dentro de una pausa de 0,5 s: se recortan 7 tramas de silencio.
    // En Python la frase siguiente empezaba 0,224 s antes de tiempo (24,8 s en vez de 25,024 s).
    let x = cat(&[tone(24.6), silence(0.5), tone(2.0), silence(1.0)]);
    let segs = run(&[(0.0, x)], SegmenterConfig::default());
    assert!(segs.len() >= 2, "{}", segs.len());
    let cut = 782.0 * 512.0 / SR; // primera trama tras el corte
    assert!(
        (segs[1].start_s - cut).abs() <= 512.0 / SR,
        "{} vs {cut}",
        segs[1].start_s
    );
}

#[test]
fn a_gap_does_not_trim_the_speech_before_it() {
    // 1 s de voz y el siguiente paquete 0,8 s después: la captura de sistema no manda paquetes en
    // silencio. El hueco cierra la frase, pero no se lleva voz (en Python se perdían ~0,55 s).
    let segs = run(
        &[(0.0, tone(1.0)), (1.8, tone(1.0))],
        SegmenterConfig::default(),
    );
    assert_eq!(segs.len(), 2);
    let kept = segs[0].audio.len() as f64 / SR;
    assert!(kept >= 0.95, "se recortó voz: quedan {kept:.3} s de 1 s");
}

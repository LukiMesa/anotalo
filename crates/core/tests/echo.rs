//! Cancelación de eco con señales sintéticas: el micrófono oye por los altavoces lo que dicen los
//! demás (con retardo, la sala y algo de ruido) y el cancelador lo resta usando lo que sonó.
//!
//! Las fuentes se simulan como las de Windows: trozos de ~21 ms con el instante de su primera
//! muestra (que llega algo tarde, al azar), y la captura del sistema que no manda nada mientras la
//! llamada calla.
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::time::Instant;

use realfft::RealFftPlanner;

use anotalo_core::echo::{
    EchoCanceller, EchoProcessor, FAR_CAP_S, FRAME, FarBuffer, JITTER_S, LEAD_S, MAX_WAIT_S, SR,
    SharedFar,
};

mod common {
    pub mod voice;
}

use common::voice::{Rng, echo, energy, mix, noise, only, rms, samples, voice};

/// Instante (reloj común) en el que empiezan las señales.
const BASE: f64 = 100.0;
const SRF: f64 = SR as f64;

/// La voz de "Otros" (lo que suena) y la de "Yo", 12 s cada una: se generan una vez.
fn far_voice() -> &'static [f32] {
    static V: OnceLock<Vec<f32>> = OnceLock::new();
    V.get_or_init(|| voice(1, 12.0, 120.0))
}

fn near_voice() -> &'static [f32] {
    static V: OnceLock<Vec<f32>> = OnceLock::new();
    V.get_or_init(|| voice(2, 12.0, 210.0))
}

/// El eco de `far_voice`: 80 ms de retardo, la sala y la mitad de fuerte.
fn far_echo() -> &'static [f32] {
    static V: OnceLock<Vec<f32>> = OnceLock::new();
    V.get_or_init(|| echo(far_voice(), 0.08, 0.5))
}

/// Cuántos dB tiene `a` más que `b`.
fn db(a: &[f32], b: &[f32]) -> f64 {
    10.0 * (energy(a) / energy(b).max(1e-20)).log10()
}

/// Correlación normalizada de `a` y `b` desplazando `b` en `lag` muestras.
fn corr_at(a: &[f32], b: &[f32], lag: i64) -> f64 {
    let n = a.len().min(b.len()) as i64;
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let j = i + lag;
        if j < 0 || j >= n {
            continue;
        }
        let (x, y) = (f64::from(a[i as usize]), f64::from(b[j as usize]));
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    ab / (aa * bb).sqrt().max(1e-20)
}

/// Espectrograma en dB: tramas de 512 muestras cada 256, de 150 Hz a 7,5 kHz. Por trama, también
/// su nivel (RMS).
fn spectrogram(x: &[f32]) -> Vec<(f32, Vec<f64>)> {
    const N: usize = 512;
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
    let window: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (TAU * i as f32 / N as f32).cos())
        .collect();
    let (lo, hi) = (150 * N / SR, 7500 * N / SR);
    let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
    (0..x.len().saturating_sub(N))
        .step_by(N / 2)
        .map(|s| {
            let frame = &x[s..s + N];
            buf.iter_mut()
                .zip(frame.iter().zip(&window))
                .for_each(|(b, (v, w))| *b = v * w);
            fft.process(&mut buf, &mut spec).unwrap();
            let db = spec[lo..hi]
                .iter()
                .map(|c| 10.0 * (f64::from(c.norm_sqr()) + 1e-10).log10())
                .collect();
            (rms(frame), db)
        })
        .collect()
}

/// Cuánto se parece lo que se oye en `out` a `reference`, sin mirar la fase (el paso alto del AEC3
/// la cambia en graves sin que se note): correlación de sus espectrogramas en dB, en las tramas en
/// las que `reference` suena.
fn likeness(reference: &[f32], out: &[f32]) -> f64 {
    let (a, b) = (spectrogram(reference), spectrogram(out));
    let (mut ab, mut aa, mut bb, mut sa, mut sb, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for ((level, x), (_, y)) in a.iter().zip(&b) {
        if *level < 0.01 {
            continue;
        }
        for (u, v) in x.iter().zip(y) {
            ab += u * v;
            aa += u * u;
            bb += v * v;
            sa += u;
            sb += v;
            n += 1.0;
        }
    }
    let cov = ab / n - (sa / n) * (sb / n);
    let va = aa / n - (sa / n).powi(2);
    let vb = bb / n - (sb / n).powi(2);
    cov / (va * vb).sqrt().max(1e-20)
}

/// El tramo [from, to) segundos de una señal.
fn span(x: &[f32], from: f64, to: f64) -> &[f32] {
    &x[samples(from)..samples(to).min(x.len())]
}

/// Cómo llegan los trozos de las dos fuentes.
#[derive(Clone, Default)]
struct Feed {
    /// Como la captura real (`winaudio::capture`): paquetes de WASAPI de 10 ms a 48 kHz, el
    /// remuestreador entrega 341–342 muestras cada 1024 de entrada (en diente de sierra, de 0 a
    /// 10 ms tarde) y el hilo despierta hasta `wake_s` tarde, al azar. El instante de cada trozo es
    /// la hora de entrega menos su duración, como allí.
    wake_s: f64,
    /// En lugar de lo anterior, más duro: cada trozo llega hasta este retraso tarde, al azar.
    random_s: f64,
    /// La captura del sistema entrega a ráfagas cada tanto (llega hasta ese tiempo tarde).
    far_burst_s: f64,
    /// Tramos (s desde el inicio) en los que la captura del sistema no manda nada.
    far_gaps: Vec<(f64, f64)>,
    /// La captura del sistema nunca manda nada.
    no_far: bool,
    /// Otra tirada de los retrasos al azar.
    seed: u64,
    /// El reloj del dispositivo de "Otros" va así de lento (ppm) respecto al del sistema.
    far_ppm: f64,
}

/// Los trozos de una fuente de `n` muestras: (llega, desde, hasta, instante). `clock`: cuánto dura
/// un segundo de su reloj en el del sistema.
fn deliveries(n: usize, feed: &Feed, rng: &mut Rng, clock: f64) -> Vec<(f64, usize, usize, f64)> {
    let mut out = Vec::new();
    if feed.random_s > 0.0 {
        let mut last = f64::NEG_INFINITY;
        for (s, e) in chunks(n) {
            let late = feed.random_s * rng.unit();
            // cada fuente entrega en orden, desde su hilo
            let at = (BASE + e as f64 / SRF * clock + late).max(last);
            last = at;
            out.push((at, s, e, BASE + s as f64 / SRF * clock + late));
        }
        return out;
    }
    let (mut pending, mut consumed, mut s, mut packet) = (0usize, 0usize, 0usize, 0usize);
    while s < n {
        packet += 1;
        pending += 480;
        if pending < 1024 {
            continue;
        }
        pending -= 1024;
        consumed += 1024;
        let e = (consumed / 3).min(n);
        let at = BASE + packet as f64 * 0.01 * clock + feed.wake_s * rng.unit();
        out.push((at, s, e, at - (e - s) as f64 / SRF));
        s = e;
    }
    out
}

/// Lo que devolvió el cancelador.
struct Run {
    audio: Vec<f32>,
    /// (instante, muestras) de cada trozo del micrófono, al entrar y al salir.
    sent: Vec<(f64, usize)>,
    got: Vec<(f64, usize)>,
}

/// Trozos de 341 o 342 muestras (lo que da el remuestreador): (desde, hasta).
fn chunks(n: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut s, mut k) = (0, 0);
    while s < n {
        let len = if k % 3 == 2 { 342 } else { 341 };
        out.push((s, (s + len).min(n)));
        s += len;
        k += 1;
    }
    out
}

/// Pasa `mic` por el cancelador con `far` como lo que sonó, entregando los trozos en orden de
/// llegada como lo harían las fuentes reales, y al final vacía lo pendiente.
fn run_with(mic: &[f32], far: &[f32], feed: &Feed, mut aec: EchoCanceller, buf: SharedFar) -> Run {
    let mut rng = Rng(5 + 7919 * feed.seed);
    // (llega, es "Otros", desde, hasta, instante)
    let mut events: Vec<(f64, bool, usize, usize, f64)> = Vec::new();
    for (is_far, n) in [(true, far.len()), (false, mic.len())] {
        if is_far && feed.no_far {
            continue;
        }
        let clock = if is_far {
            1.0 + feed.far_ppm * 1e-6
        } else {
            1.0
        };
        for (mut at, s, e, t) in deliveries(n, feed, &mut rng, clock) {
            if is_far {
                // solo calla del todo: un trozo con algo de voz sí llega
                let (from, to) = (s as f64 / SRF, e as f64 / SRF);
                if feed.far_gaps.iter().any(|(a, b)| from >= *a && to <= *b) {
                    continue;
                }
                if feed.far_burst_s > 0.0 {
                    at = (at / feed.far_burst_s).ceil() * feed.far_burst_s;
                }
            }
            events.push((at, is_far, s, e, t));
        }
    }
    // a igual llegada, primero lo que sonó (como mucho da igual: se espera)
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut out = Vec::new();
    let mut sent = Vec::new();
    for (now, is_far, s, e, t) in events {
        if is_far {
            buf.lock().unwrap().push(t, &far[s..e]);
        } else {
            sent.push((t, e - s));
            out.extend(aec.process(t, &mic[s..e], now));
        }
    }
    out.extend(aec.drain());
    let got = out.iter().map(|(t, x)| (*t, x.len())).collect();
    Run {
        audio: out.into_iter().flat_map(|(_, x)| x).collect(),
        sent,
        got,
    }
}

fn run(mic: &[f32], far: &[f32], feed: &Feed) -> Run {
    let buf = FarBuffer::shared();
    run_with(mic, far, feed, EchoCanceller::new(buf.clone()), buf)
}

/// Como la captura real, con el hilo despertando hasta 3 ms tarde.
fn jittery() -> Feed {
    Feed {
        wake_s: 0.003,
        ..Feed::default()
    }
}

/// Más duro que la captura real: cada trozo, hasta 30 ms tarde al azar.
fn shaky() -> Feed {
    Feed {
        random_s: 0.03,
        ..Feed::default()
    }
}

// --- lo que sonó ------------------------------------------------------------------------------

#[test]
fn test_far_buffer_fills_gaps_with_silence() {
    let mut b = FarBuffer::new();
    b.push(10.0, &[1.0; 160]);
    // medio segundo sin paquetes (la llamada calla) y vuelve: se coloca al juntar un poco
    b.push(10.5, &[2.0; 1600]);
    assert_eq!(b.end_s(), Some(10.01));
    b.push(10.6, &[2.0; 1600]);
    assert_eq!(b.end_s(), Some(10.7));
    let mut out = vec![9.0f32; 11200];
    b.read(10.0, &mut out);
    assert!(out[..160].iter().all(|v| *v == 1.0));
    assert!(out[160..8000].iter().all(|v| *v == 0.0));
    assert!(out[8000..].iter().all(|v| *v == 2.0));
}

#[test]
fn test_far_buffer_keeps_its_place_across_gaps() {
    // cada instante llega tarde al azar (0–10 ms); tras callar 1 s, el audio que vuelve queda donde
    // estaría si no hubiera callado, aunque su primer trozo llegue con otro retraso
    let mut b = FarBuffer::new();
    let mut rng = Rng(21);
    let push = |b: &mut FarBuffer, s: usize, late: f64| {
        let x: Vec<f32> = (s..s + 341).map(|i| i as f32).collect();
        b.push(10.0 + s as f64 / SRF + late, &x);
    };
    push(&mut b, 0, 0.008);
    for k in 1..60 {
        push(&mut b, k * 341, 0.01 * rng.unit());
    }
    let resume = 60 * 341 + SR;
    push(&mut b, resume, 0.0);
    for k in 1..10 {
        push(&mut b, resume + k * 341, 0.01 * rng.unit());
    }
    // sin callar, la muestra i estaría en 10 s + i + 8 ms (lo que tardó el primer trozo)
    let mut out = vec![0.0f32; 1];
    let i = resume + 1000;
    b.read(10.0 + (i as f64) / SRF + 0.008, &mut out);
    let off = out[0] as i64 - i as i64;
    assert!(off.abs() <= 32, "desplazado {off} muestras");
}

#[test]
fn test_far_buffer_follows_a_drifting_clock() {
    // el reloj del dispositivo va un 0,1 % más lento o más rápido que el del sistema (exagerado: los
    // de verdad, 0,002–0,01 %): en 10 minutos, 0,6 s de desfase. Lo que sonó se mantiene cerca de
    // sus instantes, sin quedarse retenido
    for ppm in [1000.0, -1000.0] {
        let mut b = FarBuffer::new();
        let mut rng = Rng(8);
        let mut worst = 0.0f64;
        let mut s = 0usize;
        for _ in 0..(600 * SR / 341) {
            // la muestra s de su reloj suena, para el del sistema, en s · (1 + ppm·10⁻⁶)
            let t = 10.0 + s as f64 / SRF * (1.0 + ppm * 1e-6) + 0.003 * rng.unit();
            b.push(t, &[0.1; 341]);
            s += 341;
            let end = b.end_s().unwrap();
            worst = worst.max((end - (t + 341.0 / SRF)).abs());
        }
        println!(
            "{ppm} ppm: lo más lejos de sus instantes, {:.0} ms",
            worst * 1000.0
        );
        assert!(
            worst <= JITTER_S + 0.02,
            "{ppm} ppm: {:.0} ms",
            worst * 1000.0
        );
    }
}

#[test]
fn test_far_buffer_keeps_jittery_chunks_contiguous() {
    // instantes que llegan hasta 30 ms tarde: el audio sigue seguido, sin huecos ni solapes
    let mut b = FarBuffer::new();
    let x: Vec<f32> = (0..3410).map(|i| i as f32).collect();
    let lates = [0.0, 0.03, 0.01, 0.025, 0.0, 0.02, 0.03, 0.005, 0.015, 0.0];
    for (k, late) in lates.iter().enumerate() {
        b.push(
            10.0 + (k * 341) as f64 / SRF + late,
            &x[k * 341..(k + 1) * 341],
        );
    }
    let mut out = vec![0.0f32; 3410];
    b.read(10.0, &mut out);
    assert_eq!(out, x);
}

#[test]
fn test_far_buffer_drops_what_arrives_too_late() {
    let mut b = FarBuffer::new();
    b.push(10.0, &[1.0; 1600]);
    let mut out = vec![0.0f32; 1600];
    // el lector ya pasó (lo que faltaba se dio por silencio) hasta 10,2 s
    b.read(10.2, &mut out);
    // un paquete que dice empezar en 10,05 s: lo anterior a 10,2 ya no sirve
    let late: Vec<f32> = (0..4800).map(|i| i as f32).collect();
    b.push(10.05, &late);
    b.read(10.2, &mut out);
    assert_eq!(out[0], 2400.0, "cada muestra en su instante");
    assert_eq!(out[1599], 3999.0);
}

#[test]
fn test_far_buffer_is_bounded() {
    let mut b = FarBuffer::new();
    for k in 0..400 {
        b.push(10.0 + k as f64 * 0.05, &[0.5; 800]);
    }
    assert!(b.len() <= samples(FAR_CAP_S), "{}", b.len());
    // lo más reciente sigue ahí
    let mut out = vec![0.0f32; 800];
    b.read(10.0 + 399.0 * 0.05, &mut out);
    assert!(out.iter().all(|v| *v == 0.5));
}

// --- el cancelador ----------------------------------------------------------------------------

#[test]
fn test_echo_only_is_cancelled() {
    let mic = mix(&[far_echo(), &noise(3, far_echo().len())]);
    for (name, feed) in [
        ("como la captura real", jittery()),
        ("con 30 ms de jitter", shaky()),
    ] {
        let r = run(&mic, far_voice(), &feed);
        let erle = db(span(&mic, 3.0, 12.0), span(&r.audio, 3.0, 12.0));
        let first = db(span(&mic, 0.0, 3.0), span(&r.audio, 0.0, 3.0));
        println!("ERLE {name}: {first:.1} dB los 3 primeros s, {erle:.1} dB desde 3 s");
        assert!(erle >= 20.0, "{name}: ERLE {erle:.1} dB");
    }
}

#[test]
fn test_near_speech_passes_untouched() {
    // la llamada calla (la captura manda ceros): la voz de "Yo" sale igual
    let near = mix(&[near_voice(), &noise(4, near_voice().len())]);
    let r = run(&near, &vec![0.0; near.len()], &jittery());
    let (loss, like) = (db(&near, &r.audio), likeness(&near, &r.audio));
    println!("solo voz cercana: {loss:.2} dB perdidos, parecido {like:.4}");
    assert!(
        loss.abs() <= 3.0 && like >= 0.95,
        "{loss:.2} dB, parecido {like:.4}"
    );
}

#[test]
fn test_output_keeps_the_timing() {
    // sin desfase: cada muestra limpia sale en el instante de la cruda (ruido blanco: el paso alto
    // apenas lo toca y la correlación marca el desfase sin ambigüedad)
    let mut rng = Rng(9);
    let x: Vec<f32> = (0..samples(3.0)).map(|_| 0.1 * rng.sym()).collect();
    let r = run(&x, &vec![0.0; x.len()], &jittery());
    let best = (-200..=200)
        .max_by(|a, b| corr_at(&x, &r.audio, *a).total_cmp(&corr_at(&x, &r.audio, *b)))
        .unwrap();
    let c = corr_at(&x, &r.audio, best);
    println!("desfase {best} muestras (correlación {c:.4})");
    assert_eq!(best, 0, "la salida va {best} muestras desplazada");
    assert!(c >= 0.9, "correlación {c:.4}");
}

#[test]
fn test_double_talk_keeps_near_speech() {
    // los dos hablan a la vez de 5 a 9 s (el eco ya se aprendió)
    let near = only(near_voice(), 5.0, 9.0);
    let mic = mix(&[far_echo(), &near, &noise(3, near.len())]);
    let r = run(&mic, far_voice(), &jittery());
    let (out, clean) = (span(&r.audio, 5.0, 9.0), span(&near, 5.0, 9.0));
    let (loss, like) = (db(clean, out), likeness(clean, out));
    let before = likeness(clean, span(&mic, 5.0, 9.0));
    let erle = db(span(&mic, 9.5, 12.0), span(&r.audio, 9.5, 12.0));
    println!(
        "doble habla: voz cercana {loss:.1} dB perdidos, parecido {like:.3} (el micrófono {before:.3}); ERLE después {erle:.1} dB"
    );
    assert!(
        like >= 0.8 && loss <= 6.0,
        "{loss:.1} dB, parecido {like:.3}"
    );
    assert!(erle >= 15.0, "ERLE después de la doble habla {erle:.1} dB");
}

#[test]
fn test_headset_keeps_near_speech() {
    // con auriculares: suena la llamada pero el micrófono no la oye. Por turnos, la voz de "Yo"
    // sale entera desde el principio
    let far = mix(&[&only(far_voice(), 0.0, 3.0), &only(far_voice(), 6.0, 9.0)]);
    let near = mix(&[
        &only(near_voice(), 3.0, 6.0),
        &only(near_voice(), 9.0, 12.0),
        &noise(4, near_voice().len()),
    ]);
    let r = run(&near, &far, &jittery());
    for (a, b) in [(3.0, 6.0), (9.0, 12.0)] {
        let (clean, out) = (span(&near, a, b), span(&r.audio, a, b));
        let (loss, like) = (db(clean, out), likeness(clean, out));
        println!("auriculares, turno de {a} a {b} s: {loss:.2} dB perdidos, parecido {like:.4}");
        assert!(
            loss <= 1.0 && like >= 0.95,
            "{loss:.2} dB, parecido {like:.4}"
        );
    }
}

#[test]
fn test_headset_overlap_turns_transparent() {
    // con auriculares y los dos hablando a la vez todo el rato: al principio el AEC3 suprime (aún
    // no sabe que no hay eco); en unos segundos lo detecta y deja pasar la voz entera
    let near = mix(&[near_voice(), &noise(4, near_voice().len())]);
    let r = run(&near, far_voice(), &jittery());
    let early = db(span(&near, 0.0, 3.0), span(&r.audio, 0.0, 3.0));
    let (clean, out) = (span(&near, 4.0, 12.0), span(&r.audio, 4.0, 12.0));
    let (loss, like) = (db(clean, out), likeness(clean, out));
    println!(
        "auriculares a la vez: {early:.1} dB perdidos los 3 primeros s; desde 4 s {loss:.2} dB, parecido {like:.4}"
    );
    assert!(
        loss <= 1.0 && like >= 0.95,
        "{loss:.2} dB, parecido {like:.4}"
    );
}

#[test]
fn test_far_gap_keeps_cancelling() {
    // la llamada calla 2 s (sin paquetes de la captura del sistema) y vuelve a sonar
    let far = only(far_voice(), 0.0, 12.0);
    let mut far = far;
    far[samples(5.0)..samples(7.0)].fill(0.0);
    let mic = mix(&[&echo(&far, 0.08, 0.5), &noise(3, far.len())]);
    let feed = Feed {
        far_gaps: vec![(5.0, 7.0)],
        ..jittery()
    };
    let r = run(&mic, &far, &feed);
    let erle = db(span(&mic, 7.2, 12.0), span(&r.audio, 7.2, 12.0));
    println!("ERLE tras 2 s sin paquetes: {erle:.1} dB");
    assert!(erle >= 20.0, "ERLE {erle:.1} dB");
    assert_eq!(r.audio.len(), mic.len());
}

#[test]
fn test_late_far_still_cancels() {
    // lo que sonó llega a ráfagas de 250 ms (hasta 250 ms tarde): el micrófono espera
    let mic = mix(&[far_echo(), &noise(3, far_echo().len())]);
    let feed = Feed {
        far_burst_s: 0.25,
        ..jittery()
    };
    let r = run(&mic, far_voice(), &feed);
    let erle = db(span(&mic, 3.0, 12.0), span(&r.audio, 3.0, 12.0));
    println!("ERLE con lo que sonó a ráfagas: {erle:.1} dB");
    assert!(erle >= 20.0, "ERLE {erle:.1} dB");
}

#[test]
fn test_missing_far_releases_mic_after_max_wait() {
    let x = &near_voice()[..3200];
    let mut aec = EchoCanceller::new(FarBuffer::shared());
    // mientras lo que sonó pueda llegar, el micrófono espera
    assert!(aec.process(BASE, &x[..1600], BASE + 0.1).is_empty());
    assert!(aec.process(BASE + 0.1, &x[1600..], BASE + 0.2).is_empty());
    assert_eq!(aec.held_since(), Some(BASE));
    // el primer trozo sale cuando se suelta la primera trama del segundo (el AEC va algo por
    // detrás): la que empieza en BASE + 0,1 s, pasado el plazo de lo que habría sonado con ella
    let free = BASE + 0.1 + LEAD_S + FRAME as f64 / SRF + MAX_WAIT_S;
    assert!(aec.poll(free - 0.005).is_empty());
    let out = aec.poll(free + 0.005);
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].0, out[0].1.len()), (BASE, 1600));
    assert_eq!(aec.held_since(), Some(BASE + 0.1));
    let rest = aec.drain();
    assert_eq!(rest.len(), 1);
    assert_eq!((rest[0].0, rest[0].1.len()), (BASE + 0.1, 1600));
    assert_eq!(aec.held_since(), None);
    assert!(aec.drain().is_empty());
}

#[test]
fn test_drain_returns_everything_in_order() {
    // trozos irregulares, instantes con jitter, eco y voz: sale todo, en orden y con su instante
    let near = only(near_voice(), 2.0, 4.0);
    let mic = mix(&[span(far_echo(), 0.0, 5.0), &near[..samples(5.0)]]);
    let r = run(&mic, span(far_voice(), 0.0, 5.0), &jittery());
    assert_eq!(r.got, r.sent);
    assert_eq!(r.audio.len(), mic.len());
}

#[test]
fn test_without_far_source_mic_passes() {
    // la captura de "Otros" no llegó a abrir: el micrófono sale igual (tras la espera)
    let near = mix(&[span(near_voice(), 0.0, 4.0), &noise(4, samples(4.0))]);
    let feed = Feed {
        no_far: true,
        ..jittery()
    };
    let r = run(&near, &[], &feed);
    assert_eq!(r.got, r.sent);
    let (loss, like) = (db(&near, &r.audio), likeness(&near, &r.audio));
    assert!(
        loss.abs() <= 3.0 && like >= 0.95,
        "{loss:.2} dB, parecido {like:.4}"
    );
}

/// Un AEC falso: retrasa la entrada `latency` muestras, como el de verdad, y revienta (o da error)
/// en la trama `fail_at`.
struct Faulty {
    line: VecDeque<f32>,
    latency: usize,
    frames: usize,
    fail_at: usize,
    panics: bool,
}

impl Faulty {
    fn new(latency: usize, fail_at: usize, panics: bool) -> Faulty {
        Faulty {
            line: std::iter::repeat_n(0.0, latency).collect(),
            latency,
            frames: 0,
            fail_at,
            panics,
        }
    }
}

impl EchoProcessor for Faulty {
    fn render(&mut self, _frame: &[f32]) -> Result<(), String> {
        Ok(())
    }

    fn capture(&mut self, frame: &[f32], out: &mut [f32]) -> Result<(), String> {
        self.frames += 1;
        if self.frames == self.fail_at {
            if self.panics {
                panic!("el AEC revienta");
            }
            return Err("el AEC falla".into());
        }
        self.line.extend(frame.iter().copied());
        for o in out.iter_mut() {
            *o = self.line.pop_front().unwrap();
        }
        Ok(())
    }

    fn latency(&self) -> usize {
        self.latency
    }
}

#[test]
fn test_failing_aec_falls_back_to_raw_mic() {
    // el AEC falso deja pasar el audio tal cual (con su retardo): pase lo que pase con él, la salida
    // es exactamente la entrada, sin perder ni desplazar una muestra
    let mic = mix(&[
        span(far_echo(), 0.0, 3.0),
        &only(near_voice(), 1.0, 2.0)[..samples(3.0)],
    ]);
    for (latency, panics) in [(64, true), (64, false), (0, true)] {
        let buf = FarBuffer::shared();
        let aec =
            EchoCanceller::with_processor(buf.clone(), Box::new(Faulty::new(latency, 120, panics)));
        let r = run_with(&mic, span(far_voice(), 0.0, 3.0), &jittery(), aec, buf);
        assert_eq!(r.got, r.sent);
        assert!(r.audio == mic, "latencia {latency}, revienta {panics}");
    }
}

/// Velocidad en release: `cargo test --release -p anotalo-core --test echo -- --ignored --nocapture`.
#[test]
#[ignore]
fn test_speed() {
    let mic = mix(&[far_echo(), &noise(3, far_echo().len())]);
    let t0 = Instant::now();
    let reps = 5;
    for _ in 0..reps {
        run(&mic, far_voice(), &Feed::default());
    }
    let audio_s = reps as f64 * mic.len() as f64 / SRF;
    let ms_per_min = t0.elapsed().as_secs_f64() * 1000.0 / (audio_s / 60.0);
    println!(
        "{ms_per_min:.0} ms por minuto de audio ({:.1} % del tiempo real)",
        ms_per_min / 600.0
    );
    assert!(ms_per_min < 6000.0, "{ms_per_min:.0} ms por minuto");
}

/// El reloj del dispositivo de "Otros" va más lento que el del sistema: 100 ppm (lo más que se
/// separa un reloj normal), 300 y 1000 (exagerado: en un minuto, 60 ms de deriva y una
/// recolocación). Un minuto de eco; lento en debug:
/// `cargo test --release -p anotalo-core --test echo -- --ignored --nocapture`.
#[test]
#[ignore]
fn test_drifting_far_clock_keeps_cancelling() {
    let far: Vec<f32> = (0..5).flat_map(|k| voice(10 + k, 12.0, 120.0)).collect();
    for ppm in [100.0, 300.0, 1000.0] {
        let clock = 1.0 + ppm * 1e-6;
        // lo que suena en el instante n del sistema es la muestra n / clock de su reloj
        let played: Vec<f32> = (0..far.len())
            .map(|n| {
                let p = n as f64 / clock;
                let (i, f) = (p.floor() as usize, (p - p.floor()) as f32);
                let next = far.get(i + 1).copied().unwrap_or(0.0);
                far[i] * (1.0 - f) + next * f
            })
            .collect();
        let mic = mix(&[&echo(&played, 0.08, 0.5), &noise(3, far.len())]);
        let feed = Feed {
            far_ppm: ppm,
            ..jittery()
        };
        let r = run(&mic, &far, &feed);
        let per: Vec<String> = (0..12)
            .map(|k| {
                let (a, b) = (k as f64 * 5.0, k as f64 * 5.0 + 5.0);
                format!("{:.0}", db(span(&mic, a, b), span(&r.audio, a, b)))
            })
            .collect();
        let erle = db(span(&mic, 3.0, 60.0), span(&r.audio, 3.0, 60.0));
        println!("reloj a {ppm} ppm: ERLE {erle:.1} dB desde 3 s; cada 5 s {per:?}");
        // más deriva que la de un reloj normal: el AEC3 la sigue peor (medido: 300 ppm, ~21 dB;
        // 1000 ppm, ~14 dB)
        if ppm <= 100.0 {
            assert!(erle >= 25.0, "{ppm} ppm: ERLE {erle:.1} dB");
        }
    }
}

/// Tras un hueco, con 20 tiradas de retrasos al azar por modelo (lento en debug):
/// `cargo test --release -p anotalo-core --test echo -- --ignored --nocapture`.
#[test]
#[ignore]
fn test_far_gap_over_many_delays() {
    let mut far = far_voice().to_vec();
    far[samples(5.0)..samples(7.0)].fill(0.0);
    let mic = mix(&[&echo(&far, 0.08, 0.5), &noise(3, far.len())]);
    let models = [
        ("captura real, hilo hasta 1 ms tarde", 0.001, 0.0),
        ("captura real, hilo hasta 3 ms tarde", 0.003, 0.0),
        ("cada trozo hasta 20 ms tarde al azar", 0.0, 0.02),
    ];
    for (name, wake_s, random_s) in models {
        let (mut first, mut rest) = (Vec::new(), Vec::new());
        for seed in 0..20 {
            let feed = Feed {
                far_gaps: vec![(5.0, 7.0)],
                wake_s,
                random_s,
                seed,
                ..Feed::default()
            };
            let r = run(&mic, &far, &feed);
            first.push(db(span(&mic, 7.0, 8.0), span(&r.audio, 7.0, 8.0)));
            rest.push(db(span(&mic, 8.0, 12.0), span(&r.audio, 8.0, 12.0)));
        }
        let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        println!(
            "{name}: ERLE el 1.er s tras el hueco {:.0} dB de media (mínimo {:.0}); después {:.0} (mínimo {:.0})",
            mean(&first),
            min(&first),
            mean(&rest),
            min(&rest)
        );
        assert!(min(&first) >= 15.0 && min(&rest) >= 25.0, "{name}");
    }
}

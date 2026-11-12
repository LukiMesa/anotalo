//! Port de tests/test_session.py (fuentes falsas, un Transcriber real con un Asr falso y el VAD
//! falso de Python), más el orden de las líneas entre las dos fuentes.
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anotalo_core::engine::{Asr, Transcriber};
use anotalo_core::models::Placement;
use anotalo_core::segmenter::VadFn;
use anotalo_core::session::{CallSession, OnLevel, OnLine, SessionConfig, SourceFactory, Speakers};
use anotalo_core::source::{AudioSource, OnAudio, now_s};
use anotalo_core::speaker::embed::Embed;
use anotalo_core::speaker::memory::StoredVoice;
use anotalo_core::speaker::naming::{CallPrints, call_prints_path};
use anotalo_whisper::Segment as Text;

mod common {
    pub mod speaker;
}

use common::speaker::basis;

const SR: usize = 16_000;

/// Transcribe cualquier frase como "texto".
struct EchoAsr;

impl Asr for EchoAsr {
    fn transcribe(&mut self, _pcm: &[f32]) -> Result<Vec<Text>, String> {
        Ok(vec![Text {
            text: " texto".into(),
            avg_logprob: -0.2,
            no_speech_prob: 0.05,
        }])
    }
}

fn transcriber() -> Arc<Transcriber> {
    let factory = Box::new(|_| Ok(Box::new(EchoAsr) as Box<dyn Asr>));
    Arc::new(Transcriber::start(Placement::Cpu, factory, Box::new(|_| {})).unwrap())
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn fake_vad() -> Box<dyn Fn() -> VadFn + Send> {
    Box::new(|| Box::new(|frame: &[f32]| if rms(frame) > 0.01 { 1.0 } else { 0.0 }))
}

#[derive(Clone, Default)]
struct Counters {
    starts: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
}

/// Entrega de golpe, al arrancar, silencio con 1 s de tono en cada momento de `speech` (a su
/// frecuencia) y 1 s de silencio al final, en trozos de 50 ms.
struct FakeSource {
    speech: Vec<(f64, f32)>,
    fail_times: usize,
    active: bool,
    c: Counters,
}

impl FakeSource {
    fn new(speech_at: &[f64]) -> FakeSource {
        FakeSource::voices(&speech_at.iter().map(|t| (*t, 220.0)).collect::<Vec<_>>())
    }

    /// Una voz distinta por frecuencia (`FakeEmbed` las distingue).
    fn voices(speech: &[(f64, f32)]) -> FakeSource {
        FakeSource {
            speech: speech.to_vec(),
            fail_times: 0,
            active: true,
            c: Counters::default(),
        }
    }
}

impl AudioSource for FakeSource {
    fn name(&self) -> &str {
        "falsa"
    }

    fn start(&mut self, mut on_audio: OnAudio) -> Result<(), String> {
        self.c.starts.fetch_add(1, Ordering::SeqCst);
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err("device gone".into());
        }
        let end = self.speech.iter().map(|(t, _)| *t).fold(0.0, f64::max) + 2.0;
        let mut x = vec![0.0f32; (end * SR as f64) as usize];
        for &(at, hz) in &self.speech {
            let from = (at * SR as f64) as usize;
            for i in 0..SR {
                x[from + i] = 0.3 * (std::f32::consts::TAU * hz * i as f32 / SR as f32).sin();
            }
        }
        let base = now_s();
        for (i, chunk) in x.chunks(800).enumerate() {
            on_audio(base + (i * 800) as f64 / SR as f64, chunk);
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.c.stopped.store(true, Ordering::SeqCst);
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

fn cfg(dir: &Path, label: &str) -> SessionConfig {
    SessionConfig {
        output_dir: dir.to_path_buf(),
        app_label: label.into(),
        label_me: "Yo".into(),
        label_others: "Otros".into(),
        silence_ms: 600,
        max_segment_s: 25.0,
        min_segment_ms: 300,
        title: None,
        speakers: None,
    }
}

fn start(
    dir: &Path,
    mic: FakeSource,
    others: Option<FakeSource>,
    others_factory: Option<SourceFactory>,
    on_line: Option<OnLine>,
    on_level: Option<OnLevel>,
) -> CallSession {
    CallSession::start(
        cfg(dir, "Manual"),
        transcriber(),
        Box::new(mic),
        others.map(|o| Box::new(o) as Box<dyn AudioSource>),
        others_factory,
        on_line,
        on_level,
        fake_vad(),
    )
    .unwrap()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn test_two_speakers_in_order() {
    let d = tempfile::tempdir().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let on_line: OnLine =
        Box::new(move |who, _hms, _t, _text| sink.lock().unwrap().push(who.to_string()));
    let sess = start(
        d.path(),
        FakeSource::new(&[1.5]),
        Some(FakeSource::new(&[3.5])),
        None,
        Some(on_line),
        None,
    );
    let text = read(&sess.stop().unwrap());
    assert!(text.contains("**[00:00:01] Yo:** texto"), "{text}");
    assert!(text.contains("**[00:00:03] Otros:** texto"), "{text}");
    assert!(text.find("Yo:**").unwrap() < text.find("Otros:**").unwrap());
    let mut who = lines.lock().unwrap().clone();
    who.sort();
    who.dedup();
    assert_eq!(who, vec!["Otros", "Yo"]);
}

#[test]
fn test_stop_flushes_pending() {
    let d = tempfile::tempdir().unwrap();
    let (mic, others) = (FakeSource::new(&[0.5]), FakeSource::new(&[0.5]));
    let (mc, oc) = (mic.c.clone(), others.c.clone());
    let sess = start(d.path(), mic, Some(others), None, None, None);
    let text = read(&sess.stop().unwrap());
    assert!(mc.stopped.load(Ordering::SeqCst) && oc.stopped.load(Ordering::SeqCst));
    assert!(
        text.contains("Yo:**") && text.contains("Otros:**"),
        "{text}"
    );
    assert!(text.contains("- Duración:"));
}

#[test]
fn test_source_failure_reopens() {
    let d = tempfile::tempdir().unwrap();
    let others = FakeSource {
        fail_times: 1,
        ..FakeSource::new(&[2.0])
    };
    let oc = others.c.clone();
    let sess = start(
        d.path(),
        FakeSource::new(&[0.5]),
        Some(others),
        None,
        None,
        None,
    );
    let text = read(&sess.stop().unwrap());
    assert_eq!(oc.starts.load(Ordering::SeqCst), 2);
    assert!(text.contains("Otros:**"), "{text}");
}

#[test]
fn test_dead_others_source_is_recreated() {
    let d = tempfile::tempdir().unwrap();
    let made = Arc::new(AtomicUsize::new(0));
    let count = made.clone();
    let factory: SourceFactory = Box::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
        Box::new(FakeSource::new(&[0.5]))
    });
    let dead = FakeSource {
        active: false,
        ..FakeSource::new(&[0.5])
    };
    let sess = start(
        d.path(),
        FakeSource::new(&[0.5]),
        Some(dead),
        Some(factory),
        None,
        None,
    );
    std::thread::sleep(Duration::from_millis(1200));
    sess.stop().unwrap();
    assert!(
        made.load(Ordering::SeqCst) > 0,
        "others_factory no se usó para recrear la fuente"
    );
}

#[test]
fn test_mic_only_session() {
    let d = tempfile::tempdir().unwrap();
    let sess = start(d.path(), FakeSource::new(&[0.5]), None, None, None, None);
    let text = read(&sess.stop().unwrap());
    assert!(text.contains("Yo:**") && !text.contains("Otros"), "{text}");
}

#[test]
fn test_plain_text_session() {
    let d = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let on_line: OnLine = Box::new(move |_, _, _, _| tx.lock().unwrap().send(()).unwrap());
    let sess = start(
        d.path(),
        FakeSource::new(&[0.5]),
        None,
        None,
        Some(on_line),
        None,
    );
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(sess.plain_text(), "texto");
    assert!(sess.text().contains("Yo:** texto"));
    assert!(sess.path().exists());
    sess.stop().unwrap();
}

/// Al parar sin esperar, lo dicho aún está por transcribir: el texto se toma tras vaciarlo.
#[test]
fn test_stop_with_text_includes_flushed_lines() {
    let d = tempfile::tempdir().unwrap();
    let sess = start(d.path(), FakeSource::new(&[0.5]), None, None, None, None);
    let (path, text) = sess.stop_with_text().unwrap();
    assert_eq!(text, "texto");
    assert!(read(&path).contains("- Duración:"));
}

#[test]
fn test_level_callback_throttled() {
    let d = tempfile::tempdir().unwrap();
    let levels = Arc::new(Mutex::new(Vec::new()));
    let sink = levels.clone();
    let on_level: OnLevel = Box::new(move |l| sink.lock().unwrap().push(l));
    let sess = start(
        d.path(),
        FakeSource::new(&[0.5]),
        None,
        None,
        None,
        Some(on_level),
    );
    sess.stop().unwrap();
    let levels = levels.lock().unwrap();
    // 2,5 s de audio en trozos de 50 ms → como mucho ~50 avisos, y se nota la voz
    assert!(levels.len() > 10 && levels.len() <= 52, "{}", levels.len());
    assert!(levels.iter().copied().fold(0.0, f32::max) > 0.1);
    assert_eq!(levels.iter().copied().fold(f32::MAX, f32::min), 0.0);
}

#[test]
fn lines_follow_speaking_order_across_lanes() {
    // El micrófono entrega todo antes de que se abra "Otros": sus frases llegan antes al
    // transcriptor, pero el archivo las ordena por cuándo se dijeron.
    let d = tempfile::tempdir().unwrap();
    let sess = start(
        d.path(),
        FakeSource::new(&[2.5, 6.5]),
        Some(FakeSource::new(&[0.5, 4.5])),
        None,
        None,
        None,
    );
    let text = read(&sess.stop().unwrap());
    let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("falta {s}:\n{text}"));
    let order = [
        at("**[00:00:00] Otros:**"),
        at("**[00:00:02] Yo:**"),
        at("**[00:00:04] Otros:**"),
        at("**[00:00:06] Yo:**"),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{text}");
}

#[test]
fn test_mark_lands_between_lines() {
    // "Yo" habla a los 0,5 s y "Otros" a los 3 s; la marca se pone hacia 1,5 s (reloj real).
    let d = tempfile::tempdir().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let on_line: OnLine =
        Box::new(move |who, _hms, _t, _text| sink.lock().unwrap().push(who.to_string()));
    let sess = start(
        d.path(),
        FakeSource::new(&[0.5]),
        Some(FakeSource::new(&[3.0])),
        None,
        Some(on_line),
        None,
    );
    std::thread::sleep(Duration::from_millis(1500));
    let at = sess.mark().expect("marca");
    assert!(at > 1.0 && at < 2.7, "{at}");
    let text = read(&sess.stop().unwrap());
    let (yo, marca, otros) = (
        text.find("Yo:**").unwrap(),
        text.find("Marca**").unwrap(),
        text.find("Otros:**").unwrap(),
    );
    assert!(yo < marca && marca < otros, "{text}");
    assert!(lines.lock().unwrap().iter().any(|w| w == "Marca"));
}

/// Distingue las voces falsas por su frecuencia (cruces por cero en la parte con sonido): bajo
/// 330 Hz es `basis(0)`, encima `basis(1)`. Menos de 0,3 s de sonido: sin huella.
struct FakeEmbed;

impl Embed for FakeEmbed {
    fn embed(&self, audio: &[f32]) -> Option<Vec<f32>> {
        let voiced: Vec<f32> = audio.iter().copied().filter(|v| v.abs() > 0.01).collect();
        if voiced.len() < SR * 3 / 10 {
            return None;
        }
        let crossings = audio
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        let hz = crossings as f32 / 2.0 / (voiced.len() as f32 / SR as f32);
        Some(basis(if hz < 330.0 { 0 } else { 1 }))
    }
}

struct NoEmbed;

impl Embed for NoEmbed {
    fn embed(&self, _audio: &[f32]) -> Option<Vec<f32>> {
        None
    }
}

/// Un modelo de voces que revienta en cada segmento.
struct PanicEmbed;

impl Embed for PanicEmbed {
    fn embed(&self, _audio: &[f32]) -> Option<Vec<f32>> {
        panic!("el modelo de voces revienta");
    }
}

fn speakers(dir: &Path, memory: Vec<StoredVoice>) -> Speakers {
    Speakers {
        embed: Arc::new(FakeEmbed),
        memory,
        prints_dir: dir.join("llamadas"),
    }
}

fn start_with(
    dir: &Path,
    mic: FakeSource,
    others: FakeSource,
    speakers: Option<Speakers>,
) -> CallSession {
    let mut c = cfg(dir, "Teams");
    c.speakers = speakers;
    CallSession::start(
        c,
        transcriber(),
        Box::new(mic),
        Some(Box::new(others)),
        None,
        None,
        None,
        fake_vad(),
    )
    .unwrap()
}

#[test]
fn test_two_remote_voices_get_personas() {
    let d = tempfile::tempdir().unwrap();
    let sess = start_with(
        d.path(),
        FakeSource::new(&[]),
        FakeSource::voices(&[(1.5, 220.0), (3.5, 440.0), (5.5, 220.0)]),
        Some(speakers(d.path(), vec![])),
    );
    let path = sess.stop().unwrap();
    let t = read(&path);
    assert!(
        t.contains("Persona 1:**") && t.contains("Persona 2:**") && !t.contains("Otros:**"),
        "{t}"
    );
    let prints: CallPrints =
        serde_json::from_str(&read(&call_prints_path(&d.path().join("llamadas"), &path))).unwrap();
    assert_eq!(prints.voices.len(), 2);
}

#[test]
fn test_one_remote_voice_stays_others() {
    let d = tempfile::tempdir().unwrap();
    let sess = start_with(
        d.path(),
        FakeSource::new(&[]),
        FakeSource::voices(&[(1.5, 220.0), (3.5, 220.0)]),
        Some(speakers(d.path(), vec![])),
    );
    let t = read(&sess.stop().unwrap());
    assert!(t.contains("Otros:**") && !t.contains("Persona"), "{t}");
}

#[test]
fn test_stored_voice_names_the_line() {
    let d = tempfile::tempdir().unwrap();
    let maria = StoredVoice {
        name: "María".into(),
        print: basis(1),
        seconds: 60.0,
    };
    let sess = start_with(
        d.path(),
        FakeSource::new(&[]),
        FakeSource::voices(&[(1.5, 440.0), (3.5, 440.0)]),
        Some(speakers(d.path(), vec![maria])),
    );
    let t = read(&sess.stop().unwrap());
    assert!(t.contains("María:**"), "{t}");
}

#[test]
fn test_embed_failure_keeps_others() {
    let d = tempfile::tempdir().unwrap();
    let mut sp = speakers(d.path(), vec![]);
    sp.embed = Arc::new(NoEmbed);
    let sess = start_with(
        d.path(),
        FakeSource::new(&[]),
        FakeSource::voices(&[(1.5, 220.0), (3.5, 440.0)]),
        Some(sp),
    );
    let path = sess.stop().unwrap();
    assert!(read(&path).contains("Otros:**"));
    assert!(!call_prints_path(&d.path().join("llamadas"), &path).exists());
}

/// Si el modelo de voces revienta, el carril de "Otros" sigue: sus frases (también las que vienen
/// después del primer fallo) llegan al archivo, sin separar.
#[test]
fn test_embed_panic_keeps_the_others_lane() {
    let d = tempfile::tempdir().unwrap();
    let mut sp = speakers(d.path(), vec![]);
    sp.embed = Arc::new(PanicEmbed);
    let sess = start_with(
        d.path(),
        FakeSource::new(&[]),
        FakeSource::voices(&[(1.5, 220.0), (5.5, 440.0)]),
        Some(sp),
    );
    let t = read(&sess.stop().unwrap());
    assert!(
        t.contains("**[00:00:01] Otros:**") && t.contains("**[00:00:05] Otros:**"),
        "{t}"
    );
    assert!(t.contains("- Duración:"), "{t}");
}

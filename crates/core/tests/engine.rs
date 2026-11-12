//! Port de los tests del trabajador de tests/test_asr.py, más whisper real con el modelo tiny y la
//! comprobación de la GPU integrada (spec §5.2).
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anotalo_core::engine::{Asr, AsrFactory, Transcriber, igpu_faster_than_cpu};
use anotalo_core::models::Placement;
use anotalo_core::segmenter::Segment;
use anotalo_whisper::Segment as Text;

/// Modelo falso: el texto sale del primer valor del audio, como en el FakeModel de Python.
struct FakeAsr {
    texts: HashMap<i32, String>,
    logprob: f32,
    nsp: f32,
    calls: Arc<AtomicUsize>,
    slow_after_first: bool,
}

impl FakeAsr {
    fn new() -> FakeAsr {
        FakeAsr {
            texts: HashMap::new(),
            logprob: -0.2,
            nsp: 0.05,
            calls: Arc::default(),
            slow_after_first: false,
        }
    }
    fn with_text(mut self, marker: f32, text: &str) -> FakeAsr {
        self.texts
            .insert((marker * 1000.0).round() as i32, text.into());
        self
    }
}

impl Asr for FakeAsr {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<Vec<Text>, String> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.slow_after_first && n > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let key = pcm.first().map_or(0, |v| (v * 1000.0).round() as i32);
        let text = self
            .texts
            .get(&key)
            .cloned()
            .unwrap_or_else(|| "hola".into());
        Ok(vec![Text {
            text: format!(" {text}"),
            avg_logprob: self.logprob,
            no_speech_prob: self.nsp,
        }])
    }
}

fn seg(marker: f32, speaker: &str) -> Segment {
    let mut audio = vec![0.0; 16_000];
    audio[0] = marker;
    Segment {
        speaker: speaker.into(),
        start_s: 0.0,
        end_s: 1.0,
        audio,
    }
}

fn factory_of(asr: FakeAsr) -> AsrFactory {
    let slot = Mutex::new(Some(asr));
    Box::new(move |_p| {
        Ok(Box::new(slot.lock().unwrap().take().expect("una sola carga")) as Box<dyn Asr>)
    })
}

fn start(asr: FakeAsr) -> Transcriber {
    Transcriber::start(Placement::Cpu, factory_of(asr), Box::new(|_| {})).unwrap()
}

/// Envía los segmentos, para el trabajador (vacía la cola) y devuelve lo que llegó.
fn collect(t: Transcriber, segs: Vec<Segment>) -> Vec<(String, String)> {
    let (tx, rx) = mpsc::channel();
    for s in segs {
        let tx = tx.clone();
        t.submit(
            s,
            Box::new(move |s, text| tx.send((s.speaker, text)).unwrap()),
        );
    }
    drop(tx);
    t.stop();
    rx.iter().collect()
}

#[test]
fn test_result_delivered() {
    assert_eq!(
        collect(start(FakeAsr::new()), vec![seg(0.5, "Yo")]),
        vec![("Yo".into(), "hola".into())]
    );
}

#[test]
fn test_hallucination_filtered() {
    let m = FakeAsr::new().with_text(0.5, "Gracias por ver");
    assert!(collect(start(m), vec![seg(0.5, "Yo")]).is_empty());
}

#[test]
fn test_low_confidence_filtered() {
    let m = FakeAsr {
        logprob: -1.5,
        nsp: 0.8,
        ..FakeAsr::new()
    };
    assert!(collect(start(m), vec![seg(0.5, "Yo")]).is_empty());
}

#[test]
fn test_fifo_order() {
    let m = FakeAsr::new()
        .with_text(0.1, "uno")
        .with_text(0.2, "dos")
        .with_text(0.3, "tres");
    let out = collect(
        start(m),
        vec![seg(0.1, "Yo"), seg(0.2, "Yo"), seg(0.3, "Yo")],
    );
    assert_eq!(
        out.into_iter().map(|x| x.1).collect::<Vec<_>>(),
        vec!["uno", "dos", "tres"]
    );
}

#[test]
fn test_fallback_on_gpu_error() {
    let placements = Arc::new(Mutex::new(Vec::new()));
    let seen = placements.clone();
    let factory: AsrFactory = Box::new(move |p| {
        seen.lock().unwrap().push(p);
        match p {
            Placement::Gpu { .. } => Err("Vulkan falló".into()),
            _ => Ok(Box::new(FakeAsr::new()) as Box<dyn Asr>),
        }
    });
    let fallbacks = Arc::new(Mutex::new(Vec::new()));
    let fb = fallbacks.clone();
    let t = Transcriber::start(
        Placement::Gpu { device: 0 },
        factory,
        Box::new(move |why| fb.lock().unwrap().push(why)),
    )
    .unwrap();
    assert_eq!(t.placement(), Placement::Cpu);
    assert_eq!(
        *placements.lock().unwrap(),
        vec![Placement::Gpu { device: 0 }, Placement::Cpu]
    );
    let fallbacks = fallbacks.lock().unwrap();
    assert_eq!(fallbacks.len(), 1);
    assert!(fallbacks[0].contains("Vulkan falló"), "{}", fallbacks[0]);
    drop(fallbacks);
    t.stop();
}

#[test]
fn test_stop_drains() {
    let out = collect(
        start(FakeAsr::new()),
        (0..5).map(|_| seg(0.5, "Yo")).collect(),
    );
    assert_eq!(out.len(), 5);
}

#[test]
fn test_wait_idle_times_out() {
    let m = FakeAsr {
        slow_after_first: true,
        ..FakeAsr::new()
    };
    let t = start(m);
    t.submit(seg(0.5, "Yo"), Box::new(|_, _| {}));
    t.submit(seg(0.5, "Yo"), Box::new(|_, _| {}));
    assert!(!t.wait_idle(Duration::from_millis(100)));
    assert!(t.wait_idle(Duration::from_secs(5)));
    t.stop();
}

#[test]
fn real_whisper_transcribes_jfk_with_tiny() {
    use anotalo_core::engine::WhisperAsr;
    use anotalo_whisper::{LoadOptions, Model, Params};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let model = root.join("target").join("models").join("ggml-tiny.bin");
    assert!(
        model.exists(),
        "falta el modelo de prueba: ejecuta scripts/get-test-model.ps1"
    );
    let factory: AsrFactory = Box::new(move |p| {
        let opts = LoadOptions {
            use_gpu: !matches!(p, Placement::Cpu),
            gpu_device: 0,
        };
        let m = Model::load(&model, &opts).map_err(|e| e.to_string())?;
        Ok(Box::new(WhisperAsr::new(
            m,
            Params {
                language: "en".into(),
                threads: 4,
            },
        )) as Box<dyn Asr>)
    });
    let placement = anotalo_core::models::choose("auto", &anotalo_whisper::devices(), None);
    let t = Transcriber::start(placement, factory, Box::new(|_| {})).unwrap();
    let mut r =
        hound::WavReader::open(root.join("third_party/whisper.cpp/samples/jfk.wav")).unwrap();
    let audio: Vec<f32> = r
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    let out = collect(
        t,
        vec![Segment {
            speaker: "Otros".into(),
            start_s: 0.0,
            end_s: 11.0,
            audio,
        }],
    );
    assert_eq!(out.len(), 1);
    assert!(
        out[0]
            .1
            .to_lowercase()
            .contains("ask not what your country"),
        "{}",
        out[0].1
    );
}

/// Fábrica falsa para la comprobación de la GPU integrada: cada transcripción adelanta el reloj
/// falso lo que se le indique (segundos), según el dispositivo.
fn timed_factory(clock: Arc<AtomicU64>, igpu_s: Option<f64>, cpu_s: f64) -> AsrFactory {
    struct Timed {
        clock: Arc<AtomicU64>,
        secs: f64,
    }
    impl Asr for Timed {
        fn transcribe(&mut self, _pcm: &[f32]) -> Result<Vec<Text>, String> {
            let now = f64::from_bits(self.clock.load(Ordering::SeqCst));
            self.clock
                .store((now + self.secs).to_bits(), Ordering::SeqCst);
            Ok(Vec::new())
        }
    }
    Box::new(move |p| match (p, igpu_s) {
        (Placement::IntegratedGpu { .. }, None) => Err("la GPU integrada falló".into()),
        (Placement::IntegratedGpu { .. }, Some(s)) => Ok(Box::new(Timed {
            clock: clock.clone(),
            secs: s,
        }) as Box<dyn Asr>),
        _ => Ok(Box::new(Timed {
            clock: clock.clone(),
            secs: cpu_s,
        }) as Box<dyn Asr>),
    })
}

#[test]
fn igpu_used_only_when_strictly_faster() {
    let clip = vec![0.0; 160_000];
    for (igpu, cpu, want) in [
        (Some(1.0), 2.0, true),
        (Some(2.0), 2.0, false),
        (Some(3.0), 2.0, false),
        (None, 2.0, false),
    ] {
        let clock = Arc::new(AtomicU64::new(0f64.to_bits()));
        let reader = clock.clone();
        let read = move || f64::from_bits(reader.load(Ordering::SeqCst));
        let f = timed_factory(clock, igpu, cpu);
        assert_eq!(
            igpu_faster_than_cpu(&f, 0, &clip, &read),
            want,
            "igpu {igpu:?} cpu {cpu}"
        );
    }
}

#[test]
fn cancel_pending_drops_queued_phrases_so_stop_is_bounded() {
    // La primera frase es rápida; las demás tardan 1 s cada una (un procesador lento).
    let m = FakeAsr {
        slow_after_first: true,
        ..FakeAsr::new()
    };
    let t = start(m);
    let (tx, rx) = mpsc::channel();
    for _ in 0..4 {
        let tx = tx.clone();
        t.submit(seg(0.5, "Yo"), Box::new(move |_, _| tx.send(()).unwrap()));
    }
    drop(tx);
    std::thread::sleep(Duration::from_millis(300)); // la 1.ª hecha, la 2.ª en curso
    assert_eq!(t.cancel_pending(), 2);
    let t0 = std::time::Instant::now();
    t.stop();
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    assert_eq!(rx.iter().count(), 2);
}

//! `anotalo record`: graba una llamada (micrófono + lo que suena) y la transcribe a Markdown.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anotalo_core::engine::{Transcriber, whisper_factory};
use anotalo_core::models::{HttpFetch, Placement, choose, models_dir};
use anotalo_core::routing::{Audio, PLATFORMS};
use anotalo_core::segmenter::VadFn;
use anotalo_core::session::{CallSession, OnLine, SessionConfig, SourceFactory};
use anotalo_core::source::OthersSpec;
use anotalo_core::vad::{FRAME, Vad};
use anotalo_whisper::DeviceKind;
use anotalo_winaudio::sources::{pick_mic, pick_others};

#[derive(Debug, Clone, PartialEq)]
pub struct RecordArgs {
    pub out: PathBuf,
    /// Etiqueta del archivo y del título (por omisión: la plataforma, "Llamada" o "Nota").
    pub app: Option<String>,
    pub platform: Option<String>,
    /// Exe cuyo audio se graba (process loopback).
    pub process: Option<String>,
    /// Solo el micrófono (nota rápida).
    pub mic_only: bool,
    pub seconds: Option<u64>,
    pub model: String,
    pub device: String,
    pub language: String,
}

impl RecordArgs {
    fn label(&self) -> String {
        if let Some(app) = &self.app {
            return app.clone();
        }
        if self.mic_only {
            return "Nota".into();
        }
        self.platform
            .as_deref()
            .and_then(|k| PLATFORMS.iter().find(|p| p.key == k))
            .map_or_else(|| "Llamada".into(), |p| p.name.to_string())
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

pub fn record(a: &RecordArgs) -> Result<(), String> {
    let missing = anotalo_whisper::missing_cpu_features();
    if !missing.is_empty() {
        return Err(anotalo_whisper::WhisperError::UnsupportedCpu(missing).to_string());
    }
    // el VAD va dentro del exe: si no carga, es un fallo de la compilación
    Vad::new().map_err(|e| format!("No se pudo cargar el detector de voz: {e}"))?;

    let devices = anotalo_whisper::devices();
    let placement = choose(&a.device, &devices, None);
    println!("Dispositivo: {}", describe(placement, &devices));
    let progress = Arc::new(|p: f32| {
        print!("\rPreparando… {:>3.0} %", p * 100.0);
        let _ = std::io::stdout().flush();
    });
    let factory = whisper_factory(
        a.model.clone(),
        a.language.clone(),
        models_dir(),
        Arc::new(HttpFetch::default()),
        progress,
    );
    let on_fallback = Box::new(|why: String| {
        eprintln!("\nAviso: la GPU falló ({why}); uso el procesador");
    });
    let transcriber = Arc::new(Transcriber::start(placement, factory, on_fallback)?);
    println!();

    let mic = pick_mic(a.platform.as_deref(), None);
    let (others, others_factory) = if a.mic_only {
        (None, None)
    } else {
        let spec = OthersSpec {
            kind: if a.process.is_some() {
                Audio::Process
            } else {
                Audio::Device
            },
            exe: a.process.clone(),
            platform: a.platform.clone(),
        };
        let first = pick_others(&spec);
        let again: SourceFactory = Box::new(move || pick_others(&spec));
        (Some(first), Some(again))
    };
    let on_line: OnLine = Box::new(|who, hms, _start, text| println!("[{hms}] {who}: {text}"));
    let cfg = SessionConfig {
        output_dir: a.out.clone(),
        app_label: a.label(),
        label_me: "Yo".into(),
        label_others: "Otros".into(),
        silence_ms: 600,
        max_segment_s: 25.0,
        min_segment_ms: 300,
    };
    let session = CallSession::start(
        cfg,
        transcriber,
        mic,
        others,
        others_factory,
        Some(on_line),
        None,
        vad_factory(),
    )
    .map_err(|e| {
        format!(
            "No se pudo crear la transcripción en {}: {e}",
            a.out.display()
        )
    })?;
    println!(
        "Grabando en {}. Pulsa Intro o Ctrl+C para terminar.",
        session.path().display()
    );
    wait_for_stop(a.seconds);
    println!("Terminando…");
    let path = session
        .stop()
        .map_err(|e| format!("No se pudo guardar la transcripción: {e}"))?;
    println!("Guardado: {}", path.display());
    Ok(())
}

fn describe(p: Placement, devices: &[anotalo_whisper::Device]) -> String {
    let gpu = |i: i32| {
        devices
            .iter()
            .filter(|d| matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu))
            .nth(i as usize)
            .map_or_else(|| format!("GPU {i}"), |d| d.description.clone())
    };
    match p {
        Placement::Gpu { device } => format!("{} (Vulkan)", gpu(device)),
        Placement::IntegratedGpu { device } => format!("{} (Vulkan, integrada)", gpu(device)),
        Placement::Cpu => "procesador".into(),
    }
}

fn vad_factory() -> Box<dyn Fn() -> VadFn + Send> {
    Box::new(|| {
        let mut vad = Vad::new().expect("el detector de voz ya cargó una vez");
        Box::new(move |frame: &[f32]| <&[f32; FRAME]>::try_from(frame).map_or(0.0, |f| vad.prob(f)))
    })
}

/// Espera a Intro, Ctrl+C o a que pasen `seconds`.
fn wait_for_stop(seconds: Option<u64>) {
    console::on_ctrl_c(|| STOP.store(true, Ordering::SeqCst));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        // sin consola (entrada cerrada) read_line devuelve 0 al momento: eso no es "Intro"
        if std::io::stdin()
            .lock()
            .read_line(&mut line)
            .is_ok_and(|n| n > 0)
        {
            let _ = tx.send(());
        }
    });
    let deadline = seconds.map(|s| Instant::now() + Duration::from_secs(s));
    while !STOP.load(Ordering::SeqCst)
        && rx.try_recv().is_err()
        && deadline.is_none_or(|d| Instant::now() < d)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
}

mod console {
    use std::sync::OnceLock;

    use windows::Win32::System::Console::SetConsoleCtrlHandler;
    use windows::core::BOOL;

    static HANDLER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

    unsafe extern "system" fn on_ctrl(_kind: u32) -> BOOL {
        if let Some(h) = HANDLER.get() {
            h();
        }
        true.into()
    }

    /// Ctrl+C (o cerrar la consola) llama a `f` en vez de matar el proceso.
    pub fn on_ctrl_c(f: impl Fn() + Send + Sync + 'static) {
        if HANDLER.set(Box::new(f)).is_ok() {
            // SAFETY: `on_ctrl` es una función estática válida mientras viva el proceso.
            let _ = unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) };
        }
    }
}

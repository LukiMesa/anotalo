//! Transcripción en un hilo propio: un modelo cargado, cola de frases, filtro de alucinaciones y
//! paso a la CPU si la GPU falla (port del trabajador de calltx/asr.py).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anotalo_whisper::{LoadOptions, Model, Params, Segment as Text};

use crate::filter::is_hallucination;
use crate::models::{Fetch, Placement, cpu_threads, ensure_model, model_for};
use crate::segmenter::Segment;

/// Un motor de transcripción cargado (whisper.cpp, o uno falso en los tests).
pub trait Asr: Send {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<Vec<Text>, String>;
}

/// Crea el motor para un dispositivo (puede descargar el modelo y tardar).
pub type AsrFactory = Box<dyn Fn(Placement) -> Result<Box<dyn Asr>, String> + Send>;

/// Recibe la frase y su texto (ya filtrado).
pub type OnResult = Box<dyn FnOnce(Segment, String) + Send>;

/// whisper.cpp con los ajustes de decodificación fijos del shim (spec §5.2).
pub struct WhisperAsr {
    model: Model,
    params: Params,
}

impl WhisperAsr {
    pub fn new(model: Model, params: Params) -> WhisperAsr {
        WhisperAsr { model, params }
    }
}

impl Asr for WhisperAsr {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<Vec<Text>, String> {
        self.model
            .transcribe(pcm, &self.params)
            .map_err(|e| e.to_string())
    }
}

/// La fábrica real: asegura el modelo del ajuste para ese dispositivo (lo descarga si hace falta),
/// lo carga y lo calienta con 1 s de silencio (la primera vez compila los shaders de Vulkan).
pub fn whisper_factory(
    settings_model: String,
    language: String,
    models_dir: PathBuf,
    fetch: Arc<dyn Fetch + Send + Sync>,
    progress: Arc<dyn Fn(f32) + Send + Sync>,
) -> AsrFactory {
    Box::new(move |p| {
        let file = model_for(&settings_model, matches!(p, Placement::Gpu { .. }));
        let path = ensure_model(file, &models_dir, fetch.as_ref(), &mut |x| progress(x))?;
        let (use_gpu, gpu_device) = match p {
            Placement::Gpu { device } | Placement::IntegratedGpu { device } => (true, device),
            Placement::Cpu => (false, 0),
        };
        let model = Model::load(
            &path,
            &LoadOptions {
                use_gpu,
                gpu_device,
            },
        )
        .map_err(|e| e.to_string())?;
        let params = Params {
            language: language.clone(),
            threads: cpu_threads(),
        };
        let mut asr = WhisperAsr::new(model, params);
        asr.transcribe(&vec![0.0; 16_000])?;
        Ok(Box::new(asr) as Box<dyn Asr>)
    })
}

struct Job {
    seg: Segment,
    on_result: OnResult,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    busy: bool,
    closing: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
}

/// Un modelo caliente y un hilo que transcribe las frases en orden de llegada.
pub struct Transcriber {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    placement: Placement,
}

impl Transcriber {
    /// Carga el motor (en este hilo) y arranca el trabajador. Si falla en la GPU, avisa una vez con
    /// `on_fallback` y usa el procesador.
    pub fn start(
        first: Placement,
        factory: AsrFactory,
        on_fallback: Box<dyn Fn(String) + Send>,
    ) -> Result<Transcriber, String> {
        let (asr, placement) = match factory(first) {
            Ok(asr) => (asr, first),
            Err(e) if first != Placement::Cpu => {
                log::warn!("No se pudo usar la GPU ({e}); paso al procesador");
                on_fallback(e);
                (factory(Placement::Cpu)?, Placement::Cpu)
            }
            Err(e) => return Err(e),
        };
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("transcripcion".into())
            .spawn(move || run(asr, &worker))
            .map_err(|e| format!("no se pudo arrancar la transcripción: {e}"))?;
        Ok(Transcriber {
            shared,
            thread: Some(thread),
            placement,
        })
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    pub fn submit(&self, seg: Segment, on_result: OnResult) {
        self.shared
            .queue
            .lock()
            .unwrap()
            .jobs
            .push_back(Job { seg, on_result });
        self.shared.changed.notify_all();
    }

    /// Espera a que todo lo enviado se haya transcrito. `false` si vence el plazo.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut q = self.shared.queue.lock().unwrap();
        while !q.jobs.is_empty() || q.busy {
            let now = Instant::now();
            if now >= deadline {
                log::warn!(
                    "Quedan {} frases sin transcribir",
                    q.jobs.len() + usize::from(q.busy)
                );
                return false;
            }
            q = self
                .shared
                .changed
                .wait_timeout(q, deadline - now)
                .unwrap()
                .0;
        }
        true
    }

    /// Descarta las frases que esperan en la cola (no la que se está transcribiendo) y dice cuántas.
    /// Para cerrar a tiempo cuando el procesador no da abasto.
    pub fn cancel_pending(&self) -> usize {
        let dropped = std::mem::take(&mut self.shared.queue.lock().unwrap().jobs).len();
        self.shared.changed.notify_all();
        dropped
    }

    /// Transcribe lo que quede en la cola y para (libera el modelo).
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.shared.queue.lock().unwrap().closing = true;
            self.shared.changed.notify_all();
            if thread.join().is_err() {
                log::error!("El hilo de transcripción terminó con un pánico");
            }
        }
    }
}

impl Drop for Transcriber {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(mut asr: Box<dyn Asr>, shared: &Shared) {
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if let Some(job) = q.jobs.pop_front() {
                    q.busy = true;
                    break job;
                }
                if q.closing {
                    return;
                }
                q = shared.changed.wait(q).unwrap();
            }
        };
        match asr.transcribe(&job.seg.audio) {
            Ok(segs) => {
                let (text, logprob, nsp) = summarize(&segs);
                if is_hallucination(&text, logprob, nsp) {
                    log::debug!("Descartado ({}): {text:?}", job.seg.speaker);
                } else {
                    (job.on_result)(job.seg, text);
                }
            }
            Err(e) => log::error!("Error transcribiendo una frase de {}: {e}", job.seg.speaker),
        }
        shared.queue.lock().unwrap().busy = false;
        shared.changed.notify_all();
    }
}

/// Texto unido sin separador y recortado; medias de confianza y de "no habla" (como Python).
fn summarize(segs: &[Text]) -> (String, f32, f32) {
    if segs.is_empty() {
        return (String::new(), 0.0, 1.0);
    }
    let text: String = segs.iter().map(|s| s.text.as_str()).collect();
    let n = segs.len() as f32;
    let logprob = segs.iter().map(|s| s.avg_logprob).sum::<f32>() / n;
    let nsp = segs.iter().map(|s| s.no_speech_prob).sum::<f32>() / n;
    (text.trim().to_string(), logprob, nsp)
}

/// ¿Es la GPU integrada estrictamente más rápida que el procesador con este audio (10 s)? La
/// fábrica debe cargar `small` en ambos. Se mide la segunda transcripción de cada uno (la primera
/// calienta). Si alguno falla, no se usa la GPU integrada.
pub fn igpu_faster_than_cpu(
    factory: &AsrFactory,
    igpu: i32,
    clip: &[f32],
    clock: &dyn Fn() -> f64,
) -> bool {
    let time = |p: Placement| -> Option<f64> {
        let mut asr = factory(p).ok()?;
        asr.transcribe(clip).ok()?;
        let t0 = clock();
        asr.transcribe(clip).ok()?;
        Some(clock() - t0)
    };
    let Some(igpu_s) = time(Placement::IntegratedGpu { device: igpu }) else {
        return false;
    };
    matches!(time(Placement::Cpu), Some(cpu_s) if igpu_s < cpu_s)
}

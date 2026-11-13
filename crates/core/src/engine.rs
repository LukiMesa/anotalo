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

/// Cada cuánto `wait_idle_until` vuelve a mirar su plazo.
const RECHECK: Duration = Duration::from_millis(100);

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
    /// El modelo ya está cargado.
    loaded: bool,
    /// La carga falló: lo que llegue se descarta.
    failed: bool,
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
    placement: Arc<Mutex<Placement>>,
}

impl Transcriber {
    /// Carga el motor y arranca el trabajador; vuelve cuando el modelo está listo. Si falla en la
    /// GPU, avisa una vez con `on_fallback` y usa el procesador.
    pub fn start(
        first: Placement,
        factory: AsrFactory,
        on_fallback: Box<dyn Fn(String) + Send>,
    ) -> Result<Transcriber, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let t = Transcriber::spawn(
            first,
            factory,
            on_fallback,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        match rx.recv() {
            Ok(Ok(_)) => Ok(t),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("la transcripción no arrancó".into()),
        }
    }

    /// Vuelve enseguida: el trabajador carga el motor (con el paso de la GPU al procesador) y
    /// después atiende la cola. Lo enviado mientras tanto espera; si la carga falla, se descarta.
    pub fn spawn(
        first: Placement,
        factory: AsrFactory,
        on_fallback: Box<dyn Fn(String) + Send>,
        on_loaded: Box<dyn FnOnce(Result<Placement, String>) + Send>,
    ) -> Transcriber {
        let shared = Arc::new(Shared::default());
        let placement = Arc::new(Mutex::new(first));
        let (worker, place) = (shared.clone(), placement.clone());
        let thread = std::thread::Builder::new()
            .name("transcripcion".into())
            .spawn(move || match load(first, &factory, &*on_fallback) {
                Ok((asr, p)) => {
                    *place.lock().unwrap() = p;
                    worker.queue.lock().unwrap().loaded = true;
                    worker.changed.notify_all();
                    on_loaded(Ok(p));
                    run(asr, &worker);
                }
                Err(e) => {
                    let dropped = {
                        let mut q = worker.queue.lock().unwrap();
                        q.failed = true;
                        std::mem::take(&mut q.jobs).len()
                    };
                    worker.changed.notify_all();
                    log::error!("No se pudo cargar el modelo ({e}); se descartan {dropped} frases");
                    on_loaded(Err(e));
                }
            })
            .ok();
        if thread.is_none() {
            shared.queue.lock().unwrap().failed = true;
        }
        Transcriber {
            shared,
            thread,
            placement,
        }
    }

    pub fn placement(&self) -> Placement {
        *self.placement.lock().unwrap()
    }

    pub fn is_loaded(&self) -> bool {
        self.shared.queue.lock().unwrap().loaded
    }

    pub fn submit(&self, seg: Segment, on_result: OnResult) {
        let mut q = self.shared.queue.lock().unwrap();
        if q.failed {
            log::warn!("Sin modelo: se descarta una frase de {}", seg.speaker);
            return;
        }
        q.jobs.push_back(Job { seg, on_result });
        drop(q);
        self.shared.changed.notify_all();
    }

    /// Espera a que todo lo enviado se haya transcrito. `false` si vence el plazo.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        self.wait_idle_until(&|| deadline)
    }

    /// Como `wait_idle`, con un plazo que puede acortarse mientras se espera (se vuelve a mirar
    /// cada `RECHECK`).
    pub fn wait_idle_until(&self, deadline: &dyn Fn() -> Instant) -> bool {
        let mut q = self.shared.queue.lock().unwrap();
        while !q.jobs.is_empty() || q.busy {
            let now = Instant::now();
            let end = deadline();
            if now >= end {
                log::warn!(
                    "Quedan {} frases sin transcribir",
                    q.jobs.len() + usize::from(q.busy)
                );
                return false;
            }
            q = self
                .shared
                .changed
                .wait_timeout(q, (end - now).min(RECHECK))
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
            let loaded = {
                let mut q = self.shared.queue.lock().unwrap();
                q.closing = true;
                q.loaded || q.failed
            };
            self.shared.changed.notify_all();
            if !loaded {
                // Aún cargando (quizá descargando): no se espera; el hilo termina al acabar.
                return;
            }
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

fn load(
    first: Placement,
    factory: &AsrFactory,
    on_fallback: &dyn Fn(String),
) -> Result<(Box<dyn Asr>, Placement), String> {
    match factory(first) {
        Ok(asr) => Ok((asr, first)),
        Err(e) if first != Placement::Cpu => {
            log::warn!("No se pudo usar la GPU ({e}); paso al procesador");
            on_fallback(e);
            Ok((factory(Placement::Cpu)?, Placement::Cpu))
        }
        Err(e) => Err(e),
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

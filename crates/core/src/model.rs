//! Vida del modelo de transcripción (spec §5.2): se prepara al arrancar (descarga, carga y
//! calentamiento, con progreso), se libera tras `release_after_min` sin grabar y vuelve a cargarse
//! en cuanto empieza una grabación, sin hacerla esperar: lo que se diga mientras tanto espera en la
//! cola. La primera vez, si solo hay GPU integrada, se mide contra el procesador.

use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

use anotalo_whisper::{Device, DeviceKind};

use crate::config::Settings;
use crate::engine::{AsrFactory, Transcriber, igpu_faster_than_cpu};
use crate::models::Placement;
use crate::notify::Notifier;

#[derive(Clone, Debug, PartialEq)]
pub enum ModelStatus {
    /// La primera preparación (bloquea las grabaciones).
    Preparing {
        progress: f32,
        text: String,
    },
    Ready,
    /// Liberado por inactividad: se vuelve a cargar al grabar, sin esperar.
    Released,
    Failed(String),
}

/// Lo que la app necesita saber del modelo antes de grabar.
pub trait ModelGate: Send + Sync {
    /// Se puede empezar a grabar ya (listo o liberado).
    fn ready(&self) -> bool;
    /// La primera preparación en curso: (progreso 0–1, texto).
    fn preparing(&self) -> Option<(f32, String)>;
    /// Reintentar si la carga falló (p. ej. sin conexión al descargar).
    fn request_reload(&self);
}

pub type Progress = Arc<dyn Fn(f32) + Send + Sync>;

pub struct ModelConfig {
    /// La fábrica del motor: `Some(modelo)` en vez del del ajuste (la medición de la GPU integrada
    /// usa `IGPU_TIMING_MODEL`), y a quién avisar del progreso de la descarga.
    pub factory: Box<dyn Fn(Option<&'static str>, Progress) -> AsrFactory + Send + Sync>,
    /// Dónde cargar, según los ajustes de ahora.
    pub placement: Box<dyn Fn() -> Placement + Send + Sync>,
    /// Segundos sin grabar tras los que se libera (0 = nunca).
    pub release_after: Arc<AtomicU64>,
    /// `igpu_check_needed` con los ajustes de ahora.
    pub igpu_check: Box<dyn Fn() -> Option<(i32, String)> + Send + Sync>,
    /// Guarda el resultado de la medición (más rápida, nombre de la GPU integrada).
    pub save_igpu: Box<dyn Fn(bool, String) + Send + Sync>,
}

struct Inner {
    status: ModelStatus,
    current: Option<Arc<Transcriber>>,
    last_used: Instant,
    preparing: bool,
}

pub struct ModelManager {
    me: Weak<ModelManager>,
    cfg: ModelConfig,
    notifier: Arc<dyn Notifier>,
    on_change: Box<dyn Fn(ModelStatus) + Send + Sync>,
    inner: Mutex<Inner>,
}

/// El modelo con el que se mide la GPU integrada contra el procesador (spec §5.2), sea cual sea el
/// elegido: el resultado se guarda una vez y vale para cualquiera.
pub const IGPU_TIMING_MODEL: &str = "small";

const GPU_FALLBACK: (&str, &str) = (
    "No se pudo preparar la GPU",
    "Esta vez uso el procesador; lo reintentaré al reiniciar.",
);
const LOAD_FAILED: (&str, &str) = (
    "Error",
    "No se pudo cargar el modelo de transcripción. Revisa app.log",
);

impl ModelManager {
    pub fn new(
        cfg: ModelConfig,
        notifier: Arc<dyn Notifier>,
        on_change: Box<dyn Fn(ModelStatus) + Send + Sync>,
    ) -> Arc<ModelManager> {
        Arc::new_cyclic(|me| ModelManager {
            me: me.clone(),
            cfg,
            notifier,
            on_change,
            inner: Mutex::new(Inner {
                status: ModelStatus::Preparing {
                    progress: 0.0,
                    text: "Cargando el modelo…".into(),
                },
                current: None,
                last_used: Instant::now(),
                preparing: false,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self, s: ModelStatus) {
        (self.on_change)(s);
    }

    pub fn status(&self) -> ModelStatus {
        self.lock().status.clone()
    }

    /// Descarga (si hace falta), carga y calienta en segundo plano. Mientras, `Preparing`.
    pub fn prepare(&self) {
        let old = {
            let mut i = self.lock();
            if i.preparing {
                return;
            }
            i.preparing = true;
            i.status = ModelStatus::Preparing {
                progress: 0.0,
                text: "Cargando el modelo…".into(),
            };
            i.current.take()
        };
        drop(old);
        self.changed(self.status());
        let Some(me) = self.me.upgrade() else { return };
        let spawned = std::thread::Builder::new()
            .name("preparar modelo".into())
            .spawn(move || me.prepare_now());
        if spawned.is_err() {
            self.lock().preparing = false;
        }
    }

    fn prepare_now(&self) {
        if let Some((device, name)) = (self.cfg.igpu_check)() {
            let factory = (self.cfg.factory)(Some(IGPU_TIMING_MODEL), self.progress());
            let clip = jfk_10s();
            let t0 = Instant::now();
            let clock = move || t0.elapsed().as_secs_f64();
            let faster = igpu_faster_than_cpu(&factory, device, &clip, &clock);
            log::info!(
                "GPU integrada {name}: {} que el procesador",
                if faster {
                    "más rápida"
                } else {
                    "no más rápida"
                }
            );
            (self.cfg.save_igpu)(faster, name);
        }
        let placement = (self.cfg.placement)();
        let factory = (self.cfg.factory)(None, self.progress());
        let (tx, rx) = mpsc::channel();
        let t = Transcriber::spawn(
            placement,
            factory,
            self.gpu_fallback(),
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        let result = rx
            .recv()
            .unwrap_or_else(|_| Err("la carga del modelo se interrumpió".into()));
        match result {
            Ok(p) => {
                log::info!("Modelo listo ({p:?})");
                let mut i = self.lock();
                i.current = Some(Arc::new(t));
                i.last_used = Instant::now();
                i.preparing = false;
                i.status = ModelStatus::Ready;
                drop(i);
                self.changed(ModelStatus::Ready);
            }
            Err(e) => {
                log::error!("No se pudo cargar el modelo: {e}");
                drop(t);
                let mut i = self.lock();
                i.preparing = false;
                i.status = ModelStatus::Failed(e.clone());
                drop(i);
                self.notifier.notify(LOAD_FAILED.0, LOAD_FAILED.1, None);
                self.changed(ModelStatus::Failed(e));
            }
        }
    }

    fn gpu_fallback(&self) -> Box<dyn Fn(String) + Send> {
        let notifier = self.notifier.clone();
        Box::new(move |_why| notifier.notify(GPU_FALLBACK.0, GPU_FALLBACK.1, None))
    }

    fn progress(&self) -> Progress {
        let me = self.me.clone();
        Arc::new(move |p| {
            if let Some(m) = me.upgrade() {
                m.on_progress(p);
            }
        })
    }

    /// Solo durante la primera preparación (la recarga tras liberar no muestra progreso).
    fn on_progress(&self, p: f32) {
        let text = if p < 1.0 {
            "Descargando el modelo…"
        } else {
            "Cargando el modelo…"
        };
        let new = ModelStatus::Preparing {
            progress: p,
            text: text.into(),
        };
        {
            let mut i = self.lock();
            if !i.preparing || i.status == new {
                return;
            }
            i.status = new.clone();
        }
        self.changed(new);
    }

    /// El motor para una grabación. Si estaba liberado, empieza a cargarse y se devuelve ya.
    pub fn acquire(&self) -> Result<Arc<Transcriber>, String> {
        let mut i = self.lock();
        match i.status.clone() {
            ModelStatus::Ready => {
                i.last_used = Instant::now();
                i.current
                    .clone()
                    .ok_or_else(|| "el modelo no está cargado".to_string())
            }
            ModelStatus::Released => {
                let t = Arc::new(self.spawn_lazy());
                i.current = Some(t.clone());
                i.status = ModelStatus::Ready;
                i.last_used = Instant::now();
                drop(i);
                log::info!("Recargando el modelo");
                self.changed(ModelStatus::Ready);
                Ok(t)
            }
            ModelStatus::Preparing { .. } => Err("Aún preparando el modelo".into()),
            ModelStatus::Failed(e) => Err(e),
        }
    }

    fn spawn_lazy(&self) -> Transcriber {
        let me = self.me.clone();
        Transcriber::spawn(
            (self.cfg.placement)(),
            (self.cfg.factory)(None, self.progress()),
            self.gpu_fallback(),
            Box::new(move |r| {
                if let (Err(e), Some(m)) = (r, me.upgrade()) {
                    m.lazy_failed(e);
                }
            }),
        )
    }

    /// Llamado desde el hilo del motor: no se suelta aquí (se esperaría a sí mismo).
    fn lazy_failed(&self, e: String) {
        log::error!("No se pudo recargar el modelo: {e}");
        self.lock().status = ModelStatus::Failed(e.clone());
        self.notifier.notify(LOAD_FAILED.0, LOAD_FAILED.1, None);
        self.changed(ModelStatus::Failed(e));
    }

    /// Cambia en vivo cuánto tiempo sin grabar libera el modelo (cero: nunca).
    pub fn set_release_after(&self, after: Duration) {
        self.cfg
            .release_after
            .store(after.as_secs(), Ordering::SeqCst);
    }

    /// Terminó una grabación.
    pub fn touch(&self, now: Instant) {
        self.lock().last_used = now;
    }

    /// Libera el modelo si lleva `release_after` sin usarse y ninguna grabación lo tiene.
    pub fn tick(&self, now: Instant) {
        let secs = self.cfg.release_after.load(Ordering::SeqCst);
        if secs == 0 {
            return;
        }
        let released = {
            let mut i = self.lock();
            let idle = now.saturating_duration_since(i.last_used) >= Duration::from_secs(secs);
            let unused = i
                .current
                .as_ref()
                .is_some_and(|t| Arc::strong_count(t) == 1);
            if i.status != ModelStatus::Ready || !idle || !unused {
                return;
            }
            i.status = ModelStatus::Released;
            i.current.take()
        };
        drop(released); // fuera del cerrojo: espera a que el motor termine y suelta la memoria
        log::info!("Modelo liberado tras {secs} s sin grabar");
        self.changed(ModelStatus::Released);
    }

    /// Dónde corre el modelo cargado; None sin modelo (preparándose, liberado o fallido).
    pub fn placement(&self) -> Option<Placement> {
        self.lock().current.as_ref().map(|t| t.placement())
    }

    /// Cambió el modelo, el dispositivo o el idioma: se suelta y se prepara otra vez.
    pub fn reload(&self) {
        self.prepare();
    }
}

impl ModelGate for ModelManager {
    fn ready(&self) -> bool {
        matches!(self.status(), ModelStatus::Ready | ModelStatus::Released)
    }

    fn preparing(&self) -> Option<(f32, String)> {
        match self.status() {
            ModelStatus::Preparing { progress, text } => Some((progress, text)),
            _ => None,
        }
    }

    fn request_reload(&self) {
        if matches!(self.status(), ModelStatus::Failed(_)) {
            self.prepare();
        }
    }
}

/// Hay que medir la GPU integrada (spec §5.2): `device` no es "cpu", no hay GPU dedicada y la
/// integrada no se ha medido aún (o es otra). Devuelve su índice para whisper.cpp y su nombre.
pub fn igpu_check_needed(s: &Settings, devices: &[Device]) -> Option<(i32, String)> {
    if s.device == "cpu" {
        return None;
    }
    let gpus: Vec<&Device> = devices
        .iter()
        .filter(|d| matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu))
        .collect();
    if gpus.iter().any(|d| d.kind == DeviceKind::Gpu) {
        return None;
    }
    let (i, igpu) = gpus
        .iter()
        .enumerate()
        .find(|(_, d)| d.kind == DeviceKind::IntegratedGpu)?;
    if s.igpu_faster.is_some() && s.igpu_name == igpu.description {
        return None;
    }
    Some((i as i32, igpu.description.clone()))
}

/// Los primeros 10 s de jfk.wav (de las muestras de whisper.cpp), para medir la GPU integrada.
fn jfk_10s() -> Vec<f32> {
    let wav: &[u8] = include_bytes!("../../../third_party/whisper.cpp/samples/jfk.wav");
    hound::WavReader::new(Cursor::new(wav))
        .map(|r| {
            r.into_samples::<i16>()
                .take(160_000)
                .filter_map(Result::ok)
                .map(|s| f32::from(s) / 32768.0)
                .collect()
        })
        .unwrap_or_else(|_| vec![0.0; 160_000])
}

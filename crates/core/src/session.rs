//! Una llamada en curso: fuentes → segmentadores → transcriptor → Markdown (port de
//! calltx/session.py).
//!
//! Cada fuente tiene su carril: una cola y un hilo con su segmentador y su VAD, así el callback de
//! audio nunca bloquea. Un reloj cada 0,5 s escribe lo que ya cumplió la espera, manda marcas de
//! tiempo (cierran frases si una fuente calla) y reabre las fuentes que se cayeron.

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::engine::Transcriber;
use crate::segmenter::{Segment, Segmenter, SegmenterConfig, VadFn};
use crate::source::{AudioSource, OnAudio, now_s};
use crate::speaker::diarizer::Diarizer;
use crate::speaker::embed::Embed;
use crate::speaker::memory::StoredVoice;
use crate::speaker::naming::save_call_prints;
use crate::transcript::{TranscriptWriter, fmt_elapsed};

/// Veces que se reabre una fuente que falla antes de darla por perdida.
pub const MAX_RETRIES: u32 = 3;
const LEVEL_INTERVAL_S: f64 = 0.05;
const TICK: Duration = Duration::from_millis(500);
const RETRY_DELAY_S: f64 = 1.0;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub struct SessionConfig {
    pub output_dir: PathBuf,
    /// Plataforma o "Llamada" (grabada a mano): va al nombre del archivo y al título.
    pub app_label: String,
    pub label_me: String,
    pub label_others: String,
    pub silence_ms: u32,
    pub max_segment_s: f64,
    pub min_segment_ms: u32,
    /// Título de la reunión (`- Título:` en la cabecera), si se conoce.
    pub title: Option<String>,
    /// Separar a cada persona de "Otros" (spec §2.4.5). None: todo es "Otros".
    pub speakers: Option<Speakers>,
}

/// Lo que hace falta para separar las voces de "Otros".
pub struct Speakers {
    pub embed: Arc<dyn Embed>,
    /// Las voces guardadas (para reconocerlas al colgar).
    pub memory: Vec<StoredVoice>,
    /// Dónde se guardan las voces de cada llamada.
    pub prints_dir: PathBuf,
}

/// (hablante, "HH:MM:SS", inicio en s, texto) de cada línea nueva.
pub type OnLine = Box<dyn Fn(&str, &str, f64, &str) + Send>;
/// Nivel (RMS) del micrófono, como mucho cada 50 ms.
pub type OnLevel = Box<dyn Fn(f32) + Send>;
/// Vuelve a buscar la fuente de "Otros" (la app de la llamada pudo reiniciarse).
pub type SourceFactory = Box<dyn Fn() -> Box<dyn AudioSource> + Send>;

enum Msg {
    Audio(f64, Vec<f32>),
    Idle(f64),
    Flush,
}

struct Lane {
    source: Box<dyn AudioSource>,
    tx: Sender<Msg>,
    failures: u32,
    level: Option<Arc<Mutex<OnLevel>>>,
}

struct Shared {
    writer: Arc<TranscriptWriter>,
    /// Inicio de la sesión según `now_s()`: los tiempos del archivo cuentan desde aquí.
    t0: f64,
    on_line: Option<Arc<Mutex<OnLine>>>,
    lanes: Mutex<Vec<Lane>>,
    stopping: Mutex<bool>,
    stop_changed: Condvar,
    others_factory: Option<Mutex<SourceFactory>>,
}

impl Shared {
    /// Espera `d` o hasta que se pida parar. true si se pidió parar.
    fn wait_stop(&self, d: Duration) -> bool {
        let stopping = self.stopping.lock().unwrap();
        *self
            .stop_changed
            .wait_timeout_while(stopping, d, |s| !*s)
            .unwrap()
            .0
    }

    /// Arranca la fuente del carril, reintentando con esperas crecientes.
    fn open(&self, lane: &mut Lane) -> bool {
        while lane.failures <= MAX_RETRIES {
            match lane
                .source
                .start(on_audio(lane.tx.clone(), lane.level.clone()))
            {
                Ok(()) => return true,
                Err(e) => {
                    lane.failures += 1;
                    log::warn!(
                        "No se pudo abrir {} ({e}), intento {}",
                        lane.source.name(),
                        lane.failures
                    );
                    let wait = Duration::from_secs_f64(RETRY_DELAY_S * f64::from(lane.failures));
                    if self.wait_stop(wait) {
                        return false;
                    }
                }
            }
        }
        log::error!(
            "Fuente {} descartada tras {MAX_RETRIES} intentos",
            lane.source.name()
        );
        false
    }

    fn tick(&self) {
        self.writer.flush_due();
        let now = now_s();
        let mut lanes = self.lanes.lock().unwrap();
        for (i, lane) in lanes.iter_mut().enumerate() {
            let _ = lane.tx.send(Msg::Idle(now));
            if !lane.source.is_active() && lane.failures <= MAX_RETRIES {
                log::warn!("La fuente {} se detuvo; reabriendo", lane.source.name());
                lane.source.stop();
                lane.failures += 1;
                if i == 1
                    && let Some(factory) = &self.others_factory
                {
                    lane.source = (factory.lock().unwrap())();
                }
                self.open(lane);
            }
        }
    }
}

/// El callback de audio de un carril: encola sin bloquear y avisa del nivel (solo el micrófono).
fn on_audio(tx: Sender<Msg>, level: Option<Arc<Mutex<OnLevel>>>) -> OnAudio {
    let mut last = f64::NEG_INFINITY;
    Box::new(move |t, x| {
        let _ = tx.send(Msg::Audio(t, x.to_vec()));
        if let Some(cb) = &level
            && t - last >= LEVEL_INTERVAL_S
        {
            last = t;
            let rms = if x.is_empty() {
                0.0
            } else {
                (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
            };
            if let Ok(cb) = cb.lock()
                && catch_unwind(AssertUnwindSafe(|| cb(rms))).is_err()
            {
                log::error!("Error notificando el nivel");
            }
        }
    })
}

/// Avisa de una línea nueva; un fallo de quien escucha no para la transcripción.
fn notify_line(cb: Option<&Arc<Mutex<OnLine>>>, speaker: &str, start_s: f64, text: &str) {
    if let Some(cb) = cb
        && let Ok(cb) = cb.lock()
    {
        let hms = fmt_elapsed(start_s);
        if catch_unwind(AssertUnwindSafe(|| cb(speaker, &hms, start_s, text))).is_err() {
            log::error!("Error notificando una línea");
        }
    }
}

type OnResult = Arc<dyn Fn(Segment, String, Option<u32>) + Send + Sync>;
/// El modelo de voces y las voces de la llamada, para el carril de "Otros".
type Diar = (Arc<dyn Embed>, Arc<Mutex<Diarizer>>);

fn lane_worker(
    rx: Receiver<Msg>,
    mut segmenter: Segmenter,
    t0: f64,
    transcriber: Arc<Transcriber>,
    on_result: OnResult,
    diar: Option<Diar>,
) {
    let submit = |segs: Vec<Segment>| {
        for mut seg in segs {
            // "Otros": la huella (fuera del cerrojo) y la etiqueta de su voz en este momento
            let voice = diar.as_ref().map(|(embed, diarizer)| {
                let emb = embed.embed(&seg.audio);
                let (id, label) = diarizer.lock().unwrap_or_else(|e| e.into_inner()).add(
                    seg.start_s,
                    seg.end_s,
                    emb,
                );
                seg.speaker = label;
                id
            });
            let cb = on_result.clone();
            transcriber.submit(seg, Box::new(move |s, text| cb(s, text, voice)));
        }
    };
    for msg in rx {
        match msg {
            Msg::Audio(t, x) => submit(segmenter.feed(t - t0, &x)),
            Msg::Idle(t) => submit(segmenter.idle(t - t0)),
            Msg::Flush => {
                submit(segmenter.flush());
                return;
            }
        }
    }
}

pub struct CallSession {
    shared: Arc<Shared>,
    transcriber: Arc<Transcriber>,
    workers: Vec<JoinHandle<()>>,
    ticker: Option<JoinHandle<()>>,
    /// Las voces de "Otros" y dónde guardarlas, si se separan.
    diarizer: Option<(Arc<Mutex<Diarizer>>, PathBuf)>,
}

impl CallSession {
    /// Abre el archivo y las fuentes y empieza a transcribir. `others = None`: nota rápida, solo el
    /// micrófono.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        cfg: SessionConfig,
        transcriber: Arc<Transcriber>,
        mic: Box<dyn AudioSource>,
        others: Option<Box<dyn AudioSource>>,
        others_factory: Option<SourceFactory>,
        on_line: Option<OnLine>,
        on_level: Option<OnLevel>,
        vad_factory: Box<dyn Fn() -> VadFn + Send>,
    ) -> io::Result<CallSession> {
        let started_at = chrono::Local::now().naive_local();
        let t0 = now_s();
        let writer = Arc::new(TranscriptWriter::new(
            &cfg.output_dir,
            &cfg.app_label,
            started_at,
            cfg.title.as_deref(),
            Box::new(now_s),
        )?);
        let on_line = on_line.map(|cb| Arc::new(Mutex::new(cb)));
        let shared = Arc::new(Shared {
            writer: writer.clone(),
            t0,
            on_line: on_line.clone(),
            lanes: Mutex::new(Vec::new()),
            stopping: Mutex::new(false),
            stop_changed: Condvar::new(),
            others_factory: others_factory.map(Mutex::new),
        });
        let on_result: OnResult =
            Arc::new(move |seg: Segment, text: String, voice: Option<u32>| {
                if writer
                    .add_voiced(&seg.speaker, seg.start_s, &text, Some(seg.end_s), voice)
                    .is_none()
                {
                    return;
                }
                notify_line(on_line.as_ref(), &seg.speaker, seg.start_s, &text);
            });

        let diar: Option<Diar> = match (&cfg.speakers, &others) {
            (Some(sp), Some(_)) => Some((
                sp.embed.clone(),
                Arc::new(Mutex::new(Diarizer::new(
                    sp.memory.clone(),
                    &cfg.label_others,
                ))),
            )),
            _ => None,
        };
        let diarizer = diar
            .as_ref()
            .zip(cfg.speakers.as_ref())
            .map(|((_, d), sp)| (d.clone(), sp.prints_dir.clone()));
        let mut sources = vec![(mic, cfg.label_me.clone(), None)];
        sources.extend(others.map(|o| (o, cfg.label_others.clone(), diar)));
        let mut level = on_level.map(|l| Arc::new(Mutex::new(l)));
        let mut workers = Vec::new();
        for (source, label, lane_diar) in sources {
            let seg_cfg = SegmenterConfig {
                silence_ms: cfg.silence_ms,
                max_segment_s: cfg.max_segment_s,
                min_segment_ms: cfg.min_segment_ms,
                ..SegmenterConfig::default()
            };
            let segmenter = Segmenter::new(&label, seg_cfg, vad_factory());
            let (tx, rx) = mpsc::channel();
            let (asr, cb) = (transcriber.clone(), on_result.clone());
            workers.push(
                std::thread::Builder::new()
                    .name(format!("carril {label}"))
                    .spawn(move || lane_worker(rx, segmenter, t0, asr, cb, lane_diar))?,
            );
            let mut lane = Lane {
                source,
                tx,
                failures: 0,
                level: level.take(), // solo el primero (el micrófono)
            };
            shared.open(&mut lane);
            shared.lanes.lock().unwrap().push(lane);
        }
        let ticker_shared = shared.clone();
        let ticker = std::thread::Builder::new()
            .name("reloj de la sesión".into())
            .spawn(move || {
                while !ticker_shared.wait_stop(TICK) {
                    ticker_shared.tick();
                }
            })?;
        Ok(CallSession {
            shared,
            transcriber,
            workers,
            ticker: Some(ticker),
            diarizer,
        })
    }

    /// Marca ahora mismo (`**[HH:MM:SS] Marca**`) y la avisa como línea "Marca". Devuelve el
    /// segundo de la llamada, o None si ya se cerró.
    pub fn mark(&self) -> Option<f64> {
        let at = (now_s() - self.shared.t0).max(0.0);
        self.shared.writer.add_mark(at)?;
        notify_line(self.shared.on_line.as_ref(), "Marca", at, "");
        Some(at)
    }

    pub fn path(&self) -> &Path {
        self.shared.writer.path()
    }

    /// Transcripción completa hasta ahora, para copiar.
    pub fn text(&self) -> String {
        self.shared.writer.text()
    }

    /// Solo lo dicho, en orden, separado por líneas en blanco.
    pub fn plain_text(&self) -> String {
        self.shared.writer.plain_texts().join("\n\n")
    }

    /// Para las fuentes, transcribe lo pendiente (hasta 60 s) y cierra el archivo.
    pub fn stop(mut self) -> io::Result<PathBuf> {
        self.shutdown()
    }

    /// Como `stop`, y además lo dicho (para copiarlo), ya con las últimas frases.
    pub fn stop_with_text(mut self) -> io::Result<(PathBuf, String)> {
        let path = self.shutdown()?;
        Ok((path, self.plain_text()))
    }

    fn shutdown(&mut self) -> io::Result<PathBuf> {
        let Some(ticker) = self.ticker.take() else {
            return Ok(self.path().to_path_buf());
        };
        *self.shared.stopping.lock().unwrap() = true;
        self.shared.stop_changed.notify_all();
        let _ = ticker.join();
        for lane in self.shared.lanes.lock().unwrap().iter_mut() {
            lane.source.stop();
            let _ = lane.tx.send(Msg::Flush);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        if !self.transcriber.wait_idle(IDLE_TIMEOUT) {
            // lo que quede ya no llegaría al archivo (se cierra ahora): no se gasta más tiempo en ello
            let dropped = self.transcriber.cancel_pending();
            log::warn!("Se cierra la transcripción con {dropped} frases sin transcribir");
        }
        // la llamada entera: etiquetas finales de cada persona y sus huellas
        let voices = self.diarizer.as_ref().map(|(d, dir)| {
            let f = d.lock().unwrap_or_else(|e| e.into_inner()).finish();
            self.shared.writer.relabel(f.labels);
            (f.voices, dir.clone())
        });
        let path = self
            .shared
            .writer
            .close(chrono::Local::now().naive_local())?;
        if let Some((voices, dir)) = voices
            && !voices.is_empty()
            && let Err(e) = save_call_prints(&dir, &path, voices)
        {
            log::warn!("No se pudieron guardar las voces de la llamada: {e}");
        }
        Ok(path)
    }
}

impl Drop for CallSession {
    fn drop(&mut self) {
        if let Err(e) = self.shutdown() {
            log::error!("No se pudo cerrar la transcripción: {e}");
        }
    }
}

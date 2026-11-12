//! Escritura incremental de la transcripción en Markdown (port de calltx/transcript.py).
//!
//! Las líneas de los dos audios pueden terminar de transcribirse en desorden, así que se retienen
//! `HOLD_S` segundos y se escriben ordenadas por su inicio. Cada escritura se sincroniza con el
//! disco: si la app se cae, lo ya escrito queda guardado. El archivo resultante es idéntico, byte a
//! byte, al de la app de Python (lo comprueba tests/transcript.rs).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::NaiveDateTime;

pub const HOLD_S: f64 = 3.0;
pub const MERGE_GAP_S: f64 = 2.0;

/// Segundos transcurridos como HH:MM:SS.
pub fn fmt_elapsed(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

struct Pending {
    speaker: String,
    start_s: f64,
    end_s: f64,
    text: String,
    arrived: f64,
}

impl Pending {
    fn render(&self) -> String {
        format!(
            "**[{}] {}:** {}",
            fmt_elapsed(self.start_s),
            self.speaker,
            self.text
        )
    }
}

struct State {
    file: Option<File>,
    pending: Vec<Pending>,
    written: Vec<String>,
    written_texts: Vec<String>,
    closed: bool,
}

pub struct TranscriptWriter {
    path: PathBuf,
    app: String,
    started_at: NaiveDateTime,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    state: Mutex<State>,
}

impl TranscriptWriter {
    pub fn new(
        dir: &Path,
        app: &str,
        started_at: NaiveDateTime,
        clock: Box<dyn Fn() -> f64 + Send + Sync>,
    ) -> io::Result<TranscriptWriter> {
        fs::create_dir_all(dir)?;
        let path = unique_path(dir, app, started_at);
        let mut file = File::create(&path)?;
        let w = TranscriptWriter {
            path,
            app: app.to_string(),
            started_at,
            clock,
            state: Mutex::new(State {
                file: None,
                pending: Vec::new(),
                written: Vec::new(),
                written_texts: Vec::new(),
                closed: false,
            }),
        };
        write_synced(&mut file, &w.header(None))?;
        w.state.lock().unwrap().file = Some(file);
        Ok(w)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn header(&self, ended_at: Option<NaiveDateTime>) -> String {
        // La llamada grabada a mano se guarda como "Llamada": en el título se lee "Manual".
        let title = if self.app == "Llamada" {
            "Manual"
        } else {
            self.app.as_str()
        };
        let mut lines = vec![
            format!(
                "# Llamada — {title} — {}",
                self.started_at.format("%Y-%m-%d %H:%M")
            ),
            String::new(),
            format!("- Inicio: {}", self.started_at.format("%H:%M:%S")),
        ];
        if let Some(end) = ended_at {
            let dur = (end - self.started_at).num_milliseconds() as f64 / 1000.0;
            lines.push(format!("- Fin: {}", end.format("%H:%M:%S")));
            lines.push(format!("- Duración: {}", fmt_elapsed(dur)));
        }
        lines.extend([String::new(), "---".into(), String::new(), String::new()]);
        lines.join("\n")
    }

    /// Añade una frase. Devuelve la línea tal como se verá, o None si el texto está vacío.
    pub fn add(
        &self,
        speaker: &str,
        start_s: f64,
        text: &str,
        end_s: Option<f64>,
    ) -> Option<String> {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return None;
        }
        let end_s = end_s.unwrap_or(start_s);
        let mut st = self.state.lock().unwrap();
        if st.closed {
            return None;
        }
        // la pendiente con el inicio más tardío (la primera si hay empate, como max() en Python)
        let mut last: Option<usize> = None;
        for (i, p) in st.pending.iter().enumerate() {
            if last.is_none_or(|j| p.start_s > st.pending[j].start_s) {
                last = Some(i);
            }
        }
        let merge = last.filter(|&j| {
            let l = &st.pending[j];
            l.speaker == speaker && start_s >= l.start_s && start_s - l.end_s < MERGE_GAP_S
        });
        match merge {
            Some(j) => {
                let l = &mut st.pending[j];
                l.text = format!("{} {}", l.text, text);
                l.end_s = l.end_s.max(end_s);
                // "arrived" se queda en la primera llegada: así una racha larga llega a disco a tiempo.
            }
            None => {
                let arrived = (self.clock)();
                st.pending.push(Pending {
                    speaker: speaker.to_string(),
                    start_s,
                    end_s,
                    text: text.clone(),
                    arrived,
                });
            }
        }
        Some(format!("**[{}] {speaker}:** {text}", fmt_elapsed(start_s)))
    }

    /// Escribe, en orden, las frases que ya cumplieron la espera.
    pub fn flush_due(&self) {
        let mut st = self.state.lock().unwrap();
        if st.closed || st.pending.is_empty() {
            return;
        }
        let now = (self.clock)();
        st.pending.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
        let n = st
            .pending
            .iter()
            .take_while(|p| now - p.arrived >= HOLD_S)
            .count();
        if n > 0 {
            let items: Vec<Pending> = st.pending.drain(..n).collect();
            // Si falla el disco se reintenta al cerrar (el texto sigue en memoria).
            let _ = emit(&mut st, &items);
        }
    }

    /// Escribe lo pendiente y reescribe el archivo con la cabecera final (fin y duración).
    pub fn close(&self, ended_at: NaiveDateTime) -> io::Result<PathBuf> {
        let mut st = self.state.lock().unwrap();
        if st.closed {
            return Ok(self.path.clone());
        }
        st.pending.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
        let items: Vec<Pending> = std::mem::take(&mut st.pending);
        let _ = emit(&mut st, &items);
        st.file = None; // cerrar antes de reemplazarlo
        let body: String = st.written.iter().map(|r| format!("{r}\n\n")).collect();
        let tmp = self.path.with_extension("tmp");
        let mut f = File::create(&tmp)?;
        write_synced(&mut f, &(self.header(Some(ended_at)) + &body))?;
        drop(f);
        fs::rename(&tmp, &self.path)?;
        st.closed = true;
        Ok(self.path.clone())
    }

    /// Transcripción completa hasta ahora (escrita + pendiente), para copiar.
    pub fn text(&self) -> String {
        let st = self.state.lock().unwrap();
        let mut lines = st.written.clone();
        lines.extend(sorted_pending(&st).iter().map(|p| p.render()));
        lines.join("\n\n")
    }

    /// Solo el texto dicho (frases ya unidas), en orden.
    pub fn plain_texts(&self) -> Vec<String> {
        let st = self.state.lock().unwrap();
        let mut texts = st.written_texts.clone();
        texts.extend(sorted_pending(&st).iter().map(|p| p.text.clone()));
        texts
    }
}

fn sorted_pending(st: &State) -> Vec<&Pending> {
    let mut v: Vec<&Pending> = st.pending.iter().collect();
    v.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
    v
}

fn emit(st: &mut State, items: &[Pending]) -> io::Result<()> {
    let rendered: Vec<String> = items.iter().map(Pending::render).collect();
    st.written.extend(rendered.iter().cloned());
    st.written_texts
        .extend(items.iter().map(|p| p.text.clone()));
    let chunk: String = rendered.iter().map(|r| format!("{r}\n\n")).collect();
    match st.file.as_mut() {
        Some(f) => write_synced(f, &chunk),
        None => Ok(()),
    }
}

fn write_synced(f: &mut File, text: &str) -> io::Result<()> {
    f.write_all(text.as_bytes())?;
    f.flush()?;
    f.sync_data()
}

fn unique_path(dir: &Path, app: &str, started_at: NaiveDateTime) -> PathBuf {
    let stem = format!("{}_{app}", started_at.format("%Y-%m-%d_%H%M"));
    let mut p = dir.join(format!("{stem}.md"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}_{n}.md"));
        n += 1;
    }
    p
}

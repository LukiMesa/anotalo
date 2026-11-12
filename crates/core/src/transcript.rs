//! Escritura incremental de la transcripción en Markdown (port de calltx/transcript.py).
//!
//! Las líneas de los dos audios pueden terminar de transcribirse en desorden, así que se retienen
//! `HOLD_S` segundos y se escriben ordenadas por su inicio. Cada escritura se sincroniza con el
//! disco: si la app se cae, lo ya escrito queda guardado. El archivo resultante es idéntico, byte a
//! byte, al de la app de Python (lo comprueba tests/transcript.rs).

use std::collections::HashMap;
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
    /// Una marca (`**[HH:MM:SS] Marca**`), no una frase.
    mark: bool,
    /// Los segmentos de voz de "Otros" que forman la línea (para la etiqueta final).
    voices: Vec<u32>,
}

impl Pending {
    fn render(&self) -> String {
        if self.mark {
            return format!("**[{}] Marca**", fmt_elapsed(self.start_s));
        }
        format!(
            "**[{}] {}:** {}",
            fmt_elapsed(self.start_s),
            self.speaker,
            self.text
        )
    }
}

/// Una línea ya escrita en el archivo.
#[derive(Clone)]
struct Row {
    line: String,
    speaker: String,
    text: String,
    start_s: f64,
    end_s: f64,
    mark: bool,
    voices: Vec<u32>,
}

impl Row {
    fn render(&mut self) {
        self.line = format!(
            "**[{}] {}:** {}",
            fmt_elapsed(self.start_s),
            self.speaker,
            self.text
        );
    }
}

/// El orden final (índices): las frases tal como se escribieron y cada marca justo antes de la
/// primera frase que empezó después de ella. Una frase larga llega al archivo al terminar de
/// transcribirse, a veces después de una marca pulsada mientras se decía: la marca va detrás.
fn final_order(rows: &[Row]) -> Vec<usize> {
    let mut marks: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].mark).collect();
    marks.sort_by(|&a, &b| rows[a].start_s.total_cmp(&rows[b].start_s));
    let mut marks = marks.into_iter().peekable();
    let mut out = Vec::with_capacity(rows.len());
    for i in (0..rows.len()).filter(|&i| !rows[i].mark) {
        while let Some(m) = marks.next_if(|&m| rows[m].start_s < rows[i].start_s) {
            out.push(m);
        }
        out.push(i);
    }
    out.extend(marks);
    out
}

fn in_final_order(rows: &[Row]) -> Vec<&str> {
    final_order(rows)
        .into_iter()
        .map(|i| rows[i].line.as_str())
        .collect()
}

/// Las líneas con las etiquetas finales de las voces (spec §2.4.5). Cada línea con voces toma la
/// etiqueta de la primera; si así no cambia ninguna, el archivo queda exactamente igual. Si cambia
/// alguna, las líneas seguidas de la misma persona (sin nada entre ellas y a menos de
/// `MERGE_GAP_S`) se unen, como en vivo.
fn relabeled(rows: &[Row], labels: &HashMap<u32, String>) -> Option<Vec<String>> {
    let new_speaker = |r: &Row| -> Option<String> {
        let first = r.voices.first()?;
        let l = labels.get(first)?;
        (!r.mark && *l != r.speaker).then(|| l.clone())
    };
    if !rows.iter().any(|r| new_speaker(r).is_some()) {
        return None;
    }
    let mut out: Vec<Row> = Vec::with_capacity(rows.len());
    for i in final_order(rows) {
        let mut r = rows[i].clone();
        if let Some(s) = new_speaker(&r) {
            r.speaker = s;
            r.render();
        }
        let joins = out.last().is_some_and(|p| {
            !r.mark
                && !p.mark
                && !r.voices.is_empty()
                && !p.voices.is_empty()
                && p.speaker == r.speaker
                && r.start_s >= p.start_s
                && r.start_s - p.end_s < MERGE_GAP_S
        });
        if joins {
            let p = out.last_mut().expect("hay una anterior");
            p.text = format!("{} {}", p.text, r.text);
            p.end_s = p.end_s.max(r.end_s);
            p.voices.extend(r.voices);
            p.render();
        } else {
            out.push(r);
        }
    }
    Some(out.into_iter().map(|r| r.line).collect())
}

struct State {
    file: Option<File>,
    pending: Vec<Pending>,
    written: Vec<Row>,
    written_texts: Vec<String>,
    /// Etiquetas finales de las voces, para `close`.
    labels: Option<HashMap<u32, String>>,
    closed: bool,
}

pub struct TranscriptWriter {
    path: PathBuf,
    app: String,
    started_at: NaiveDateTime,
    /// Título de la reunión (`- Título:`), en una línea.
    title: Option<String>,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    state: Mutex<State>,
}

impl TranscriptWriter {
    pub fn new(
        dir: &Path,
        app: &str,
        started_at: NaiveDateTime,
        title: Option<&str>,
        clock: Box<dyn Fn() -> f64 + Send + Sync>,
    ) -> io::Result<TranscriptWriter> {
        fs::create_dir_all(dir)?;
        let path = unique_path(dir, app, started_at);
        let mut file = File::create(&path)?;
        let w = TranscriptWriter {
            path,
            app: app.to_string(),
            started_at,
            title: title
                .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|t| !t.is_empty()),
            clock,
            state: Mutex::new(State {
                file: None,
                pending: Vec::new(),
                written: Vec::new(),
                written_texts: Vec::new(),
                labels: None,
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
        // La app de Python ignora las líneas de cabecera que no conoce.
        if let Some(t) = &self.title {
            lines.push(format!("- Título: {t}"));
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
        self.add_voiced(speaker, start_s, text, end_s, None)
    }

    /// Como `add`, con el segmento de voz de "Otros" del que sale la frase (su etiqueta final se
    /// pone al cerrar, con `relabel`).
    pub fn add_voiced(
        &self,
        speaker: &str,
        start_s: f64,
        text: &str,
        end_s: Option<f64>,
        voice: Option<u32>,
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
            !l.mark
                && l.speaker == speaker
                && start_s >= l.start_s
                && start_s - l.end_s < MERGE_GAP_S
        });
        match merge {
            Some(j) => {
                let l = &mut st.pending[j];
                l.text = format!("{} {}", l.text, text);
                l.end_s = l.end_s.max(end_s);
                l.voices.extend(voice);
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
                    mark: false,
                    voices: voice.into_iter().collect(),
                });
            }
        }
        Some(format!("**[{}] {speaker}:** {text}", fmt_elapsed(start_s)))
    }

    /// Marca en `at_s` (`**[HH:MM:SS] Marca**`): se ordena con las frases, nunca se une a ellas y
    /// corta la unión de dos frases del mismo hablante. None si ya está cerrado.
    pub fn add_mark(&self, at_s: f64) -> Option<String> {
        let mut st = self.state.lock().unwrap();
        if st.closed {
            return None;
        }
        let arrived = (self.clock)();
        let mark = Pending {
            speaker: String::new(),
            start_s: at_s,
            end_s: at_s,
            text: String::new(),
            arrived,
            mark: true,
            voices: Vec::new(),
        };
        let line = mark.render();
        st.pending.push(mark);
        Some(line)
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

    /// Las etiquetas finales de las voces de "Otros" (por segmento), que `close` aplica.
    pub fn relabel(&self, labels: HashMap<u32, String>) {
        self.state.lock().unwrap().labels = Some(labels);
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
        let lines: Vec<String> = match st.labels.as_ref().and_then(|l| relabeled(&st.written, l)) {
            Some(lines) => lines,
            None => in_final_order(&st.written)
                .into_iter()
                .map(str::to_string)
                .collect(),
        };
        let body: String = lines.iter().map(|r| format!("{r}\n\n")).collect();
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
        let mut lines: Vec<String> = in_final_order(&st.written)
            .into_iter()
            .map(str::to_string)
            .collect();
        lines.extend(sorted_pending(&st).iter().map(|p| p.render()));
        lines.join("\n\n")
    }

    /// Solo el texto dicho (frases ya unidas), en orden.
    pub fn plain_texts(&self) -> Vec<String> {
        let st = self.state.lock().unwrap();
        let mut texts = st.written_texts.clone();
        texts.extend(
            sorted_pending(&st)
                .iter()
                .filter(|p| !p.mark)
                .map(|p| p.text.clone()),
        );
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
    st.written
        .extend(items.iter().zip(&rendered).map(|(p, line)| Row {
            line: line.clone(),
            speaker: p.speaker.clone(),
            text: p.text.clone(),
            start_s: p.start_s,
            end_s: p.end_s,
            mark: p.mark,
            voices: p.voices.clone(),
        }));
    st.written_texts
        .extend(items.iter().filter(|p| !p.mark).map(|p| p.text.clone()));
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

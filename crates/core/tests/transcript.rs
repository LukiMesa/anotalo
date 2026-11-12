//! Port de tests/test_transcript.py, más la comparación byte a byte con la app de Python.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anotalo_core::transcript::TranscriptWriter;
use chrono::{NaiveDate, NaiveDateTime};

fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, s)
        .unwrap()
}

fn start() -> NaiveDateTime {
    dt(2026, 10, 23, 15, 4, 12)
}

fn end() -> NaiveDateTime {
    dt(2026, 10, 23, 15, 41, 3)
}

/// Reloj falso que el test adelanta a mano.
#[derive(Clone, Default)]
struct Clock(Arc<AtomicU64>);

impl Clock {
    fn set(&self, t: f64) {
        self.0.store(t.to_bits(), Ordering::SeqCst);
    }
    fn boxed(&self) -> Box<dyn Fn() -> f64 + Send + Sync> {
        let c = self.0.clone();
        Box::new(move || f64::from_bits(c.load(Ordering::SeqCst)))
    }
}

fn make(dir: &tempfile::TempDir, app: &str) -> (TranscriptWriter, Clock) {
    let clock = Clock::default();
    (
        TranscriptWriter::new(dir.path(), app, start(), None, clock.boxed()).unwrap(),
        clock,
    )
}

fn read(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn test_filename() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    assert_eq!(w.path().file_name().unwrap(), "2026-10-23_1504_Teams.md");
}

#[test]
fn test_line_format() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add("Yo", 5.2, "Hola", None);
    assert!(read(&w.close(end()).unwrap()).contains("**[00:00:05] Yo:** Hola"));
}

#[test]
fn test_reorders_by_start() {
    let d = tempfile::tempdir().unwrap();
    let (w, clock) = make(&d, "Teams");
    w.add("Otros", 10.0, "segundo", Some(11.0));
    w.add("Yo", 8.0, "primero", Some(9.0));
    clock.set(10.0);
    w.flush_due();
    let text = read(w.path());
    assert!(text.find("primero").unwrap() < text.find("segundo").unwrap());
}

#[test]
fn test_merge_same_speaker_short_gap() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add("Otros", 1.0, "Necesito", Some(2.0));
    w.add("Otros", 2.5, "un botón azul.", Some(4.0));
    let text = read(&w.close(end()).unwrap());
    assert!(text.contains("**[00:00:01] Otros:** Necesito un botón azul."));
    assert_eq!(text.matches("Otros:**").count(), 1);
}

#[test]
fn test_no_merge_long_gap() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add("Otros", 1.0, "Uno", Some(2.0));
    w.add("Otros", 6.0, "Dos", Some(7.0));
    assert_eq!(
        read(&w.close(end()).unwrap()).matches("Otros:**").count(),
        2
    );
}

#[test]
fn test_empty_text_ignored() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    assert!(w.add("Yo", 1.0, "   ", None).is_none());
    assert!(!read(&w.close(end()).unwrap()).contains("Yo:**"));
}

#[test]
fn test_lines_on_disk_before_close() {
    let d = tempfile::tempdir().unwrap();
    let (w, clock) = make(&d, "Teams");
    w.add("Yo", 1.0, "Hola cliente", Some(2.0));
    clock.set(3.5);
    w.flush_due();
    assert!(read(w.path()).contains("**[00:00:01] Yo:** Hola cliente"));
}

#[test]
fn test_not_flushed_before_hold() {
    let d = tempfile::tempdir().unwrap();
    let (w, clock) = make(&d, "Teams");
    w.add("Yo", 1.0, "pronto", Some(2.0));
    clock.set(1.0);
    w.flush_due();
    assert!(!read(w.path()).contains("pronto"));
}

#[test]
fn test_close_writes_duration() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    let text = read(&w.close(end()).unwrap());
    assert!(text.contains("# Llamada — Teams — 2026-10-23 15:04"));
    assert!(text.contains("- Inicio: 15:04:12"));
    assert!(text.contains("- Fin: 15:41:03"));
    assert!(text.contains("- Duración: 00:36:51"));
}

#[test]
fn test_name_collision() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = make(&d, "Teams");
    let (b, _) = make(&d, "Teams");
    assert_ne!(a.path(), b.path());
    assert_eq!(b.path().file_name().unwrap(), "2026-10-23_1504_Teams_2.md");
}

#[test]
fn test_long_merged_run_reaches_disk() {
    // El cliente habla 60 s con pausas cortas: lo dicho debe llegar a disco sin esperar a que calle.
    let d = tempfile::tempdir().unwrap();
    let (w, clock) = make(&d, "Teams");
    for i in 0..30 {
        clock.set(i as f64 * 2.0);
        w.add(
            "Otros",
            i as f64 * 2.0,
            &format!("frase{i}"),
            Some(i as f64 * 2.0 + 1.5),
        );
        w.flush_due();
    }
    assert!(read(w.path()).contains("frase0"));
}

#[test]
fn test_manual_call_header_says_manual() {
    let d = tempfile::tempdir().unwrap();
    let w = TranscriptWriter::new(
        d.path(),
        "Llamada",
        dt(2026, 11, 11, 11, 43, 0),
        None,
        Box::new(|| 0.0),
    )
    .unwrap();
    let path = w.close(dt(2026, 11, 11, 11, 44, 0)).unwrap();
    assert_eq!(
        read(&path).lines().next().unwrap(),
        "# Llamada — Manual — 2026-11-11 11:43"
    );
    assert_eq!(path.file_name().unwrap(), "2026-11-11_1143_Llamada.md");
}

#[test]
fn golden_matches_python_byte_for_byte() {
    let d = tempfile::tempdir().unwrap();
    let clock = Clock::default();
    let w = TranscriptWriter::new(
        d.path(),
        "Teams",
        dt(2026, 11, 11, 9, 5, 7),
        None,
        clock.boxed(),
    )
    .unwrap();
    w.add("Otros", 1.2, "Hola, ¿me oyes?", Some(2.4));
    w.add("Yo", 3.0, "Sí, perfecto.", Some(3.8));
    w.add("Yo", 4.5, "Empezamos.", Some(5.0));
    w.add("Otros", 30.0, "Vale.", None);
    clock.set(10.0);
    w.flush_due();
    let got = std::fs::read(w.close(dt(2026, 11, 11, 9, 6, 0)).unwrap()).unwrap();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transcript_python.md");
    let want = std::fs::read(fixture).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&got),
        String::from_utf8_lossy(&want)
    );
    assert_eq!(got, want);
}

#[test]
fn accented_output_dir() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("José").join("Llamadas");
    let w = TranscriptWriter::new(&dir, "Teams", start(), None, Box::new(|| 0.0)).unwrap();
    w.add("Yo", 1.0, "Hola", None);
    let path = w.close(end()).unwrap();
    assert!(path.starts_with(&dir) && read(&path).contains("Hola"));
}

fn titled(dir: &tempfile::TempDir, title: &str) -> (TranscriptWriter, Clock) {
    let clock = Clock::default();
    (
        TranscriptWriter::new(dir.path(), "Teams", start(), Some(title), clock.boxed()).unwrap(),
        clock,
    )
}

#[test]
fn test_header_with_title() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = titled(&d, "Reunión con Acme");
    let open: Vec<String> = read(w.path()).lines().map(String::from).collect();
    assert_eq!(
        open[..6],
        [
            "# Llamada — Teams — 2026-10-23 15:04",
            "",
            "- Inicio: 15:04:12",
            "- Título: Reunión con Acme",
            "",
            "---"
        ]
    );
    let closed: Vec<String> = read(&w.close(end()).unwrap())
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(
        closed[2..6],
        [
            "- Inicio: 15:04:12",
            "- Fin: 15:41:03",
            "- Duración: 00:36:51",
            "- Título: Reunión con Acme"
        ]
    );
}

#[test]
fn test_title_with_newline_is_one_line() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = titled(&d, " Reunión\ncon  Acme ");
    assert!(read(w.path()).contains("\n- Título: Reunión con Acme\n"));
}

#[test]
fn test_mark_line_format() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    assert_eq!(w.add_mark(65.4).as_deref(), Some("**[00:01:05] Marca**"));
    assert!(read(&w.close(end()).unwrap()).contains("\n**[00:01:05] Marca**\n\n"));
    assert_eq!(w.add_mark(70.0), None, "cerrado: no se marca");
}

#[test]
fn test_mark_breaks_same_speaker_merge() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add("Otros", 1.0, "Necesito", Some(2.0));
    w.add_mark(2.2);
    w.add("Otros", 2.5, "un botón azul.", Some(4.0));
    let text = read(&w.close(end()).unwrap());
    assert_eq!(text.matches("Otros:**").count(), 2, "{text}");
}

#[test]
fn test_mark_not_in_plain_texts() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add("Yo", 1.0, "Hola", Some(2.0));
    w.add_mark(3.0);
    assert_eq!(w.plain_texts(), vec!["Hola"]);
    assert!(w.text().contains("**[00:00:03] Marca**"));
}

#[test]
fn test_marks_ordered_by_time() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add("Otros", 10.0, "segundo", Some(11.0));
    w.add_mark(5.0);
    w.add("Yo", 2.0, "primero", Some(3.0));
    let text = read(&w.close(end()).unwrap());
    let (a, b, c) = (
        text.find("primero").unwrap(),
        text.find("Marca").unwrap(),
        text.find("segundo").unwrap(),
    );
    assert!(a < b && b < c, "{text}");
}

/// Una frase larga que se marca a la mitad llega al archivo después de la marca (se transcribe al
/// acabar): al cerrar, la marca va detrás de la frase que estaba en curso, no antes.
#[test]
fn test_mark_written_before_a_late_line_that_started_earlier() {
    let d = tempfile::tempdir().unwrap();
    let (w, clock) = make(&d, "Teams");
    clock.set(19.0);
    w.add_mark(16.0); // pulsado a los 2 s de una frase que empezó en el 14
    clock.set(22.5);
    w.flush_due(); // la marca ya está en disco
    clock.set(27.0);
    w.add(
        "Otros",
        14.0,
        "Estamos de acuerdo en el precio.",
        Some(26.0),
    );
    w.add("Yo", 27.0, "Perfecto.", Some(28.0));
    clock.set(31.0);
    w.flush_due();
    let text = read(&w.close(end()).unwrap());
    let (line, mark, after) = (
        text.find("Estamos de acuerdo").unwrap(),
        text.find("Marca").unwrap(),
        text.find("Perfecto").unwrap(),
    );
    assert!(line < mark && mark < after, "{text}");
    assert!(w.text().find("Estamos").unwrap() < w.text().find("Marca").unwrap());
}

#[test]
fn test_rust_fixture_with_title_and_marks() {
    let d = tempfile::tempdir().unwrap();
    let clock = Clock::default();
    let w = TranscriptWriter::new(
        d.path(),
        "Teams",
        dt(2026, 11, 11, 9, 5, 7),
        Some("Reunión con Acme"),
        clock.boxed(),
    )
    .unwrap();
    w.add("Otros", 1.2, "Hola, ¿empezamos?", Some(1.9));
    w.add_mark(2.5);
    w.add("Yo", 3.0, "Sí, adelante.", Some(3.8));
    let got = std::fs::read(w.close(dt(2026, 11, 11, 9, 6, 0)).unwrap()).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transcript_rust_title_marks.md");
    let want = std::fs::read(fixture).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&got),
        String::from_utf8_lossy(&want)
    );
}

fn labels(pairs: &[(u32, &str)]) -> HashMap<u32, String> {
    pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
}

#[test]
fn test_relabel_renames_rows_at_close() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add_voiced("Otros", 1.0, "Hola.", Some(2.0), Some(0));
    w.add("Yo", 5.0, "Buenas.", Some(6.0));
    w.add_voiced("Persona 2", 9.0, "Yo también.", Some(10.0), Some(1));
    w.relabel(labels(&[(0, "María"), (1, "Persona 1")]));
    let t = read(&w.close(end()).unwrap());
    assert!(
        t.contains("**[00:00:01] María:** Hola.")
            && t.contains("**[00:00:09] Persona 1:** Yo también."),
        "{t}"
    );
}

#[test]
fn test_relabel_merges_rows_that_become_the_same_person() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add_voiced("Otros", 1.0, "Hola", Some(3.0), Some(0));
    w.add_voiced("Persona 2", 3.5, "qué tal", Some(5.0), Some(1));
    w.relabel(labels(&[(0, "Otros"), (1, "Otros")]));
    let t = read(&w.close(end()).unwrap());
    assert!(t.contains("**[00:00:01] Otros:** Hola qué tal"), "{t}");
    assert_eq!(t.matches("Otros:**").count(), 1);
}

/// Dos segmentos unidos en vivo (la misma etiqueta y poca pausa) que al final son dos personas:
/// cada una se queda con su texto, en su propia línea y con su hora.
#[test]
fn test_relabel_splits_a_live_merged_row_of_two_people() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add_voiced("Otros", 1.0, "Hola", Some(2.0), Some(0));
    w.add_voiced("Otros", 2.5, "soy Juan", Some(3.5), Some(1)); // se une en vivo
    assert_eq!(w.plain_texts(), vec!["Hola soy Juan"]);
    w.relabel(labels(&[(0, "María"), (1, "Persona 1")]));
    let t = read(&w.close(end()).unwrap());
    assert!(
        t.contains("\n**[00:00:01] María:** Hola\n\n**[00:00:02] Persona 1:** soy Juan\n\n"),
        "{t}"
    );
}

/// Los trozos que salen de partir una línea se unen después con la línea vecina de la misma persona.
#[test]
fn test_split_parts_merge_with_their_neighbours() {
    let d = tempfile::tempdir().unwrap();
    let (w, clock) = make(&d, "Teams");
    w.add_voiced("Otros", 1.0, "Uno.", Some(2.0), Some(0));
    clock.set(10.0);
    w.flush_due(); // ya escrita: la siguiente no se une en vivo
    w.add_voiced("Otros", 2.5, "Dos.", Some(3.0), Some(1));
    w.add_voiced("Otros", 4.0, "Tres.", Some(5.0), Some(2)); // se une a "Dos." en vivo
    w.relabel(labels(&[(0, "María"), (1, "María"), (2, "Juan")]));
    let t = read(&w.close(end()).unwrap());
    assert!(
        t.contains("\n**[00:00:01] María:** Uno. Dos.\n\n**[00:00:04] Juan:** Tres.\n\n"),
        "{t}"
    );
}

#[test]
fn test_relabel_without_changes_is_byte_identical() {
    let run = |relabel: bool| {
        let d = tempfile::tempdir().unwrap();
        let (w, clock) = make(&d, "Teams");
        w.add_voiced("Otros", 1.0, "Uno.", Some(2.0), Some(0));
        clock.set(10.0);
        w.flush_due();
        // no se une en vivo: la primera ya estaba escrita
        w.add_voiced("Otros", 2.5, "Dos.", Some(3.0), Some(1));
        if relabel {
            w.relabel(labels(&[(0, "Otros"), (1, "Otros")]));
        }
        std::fs::read(w.close(end()).unwrap()).unwrap()
    };
    assert_eq!(run(true), run(false));
}

#[test]
fn test_mark_still_breaks_merges_after_relabel() {
    let d = tempfile::tempdir().unwrap();
    let (w, _) = make(&d, "Teams");
    w.add_voiced("Otros", 1.0, "Antes", Some(2.0), Some(0));
    w.add_mark(2.2);
    w.add_voiced("Persona 2", 2.5, "después", Some(4.0), Some(1));
    w.relabel(labels(&[(0, "Persona 1"), (1, "Persona 1")]));
    let t = read(&w.close(end()).unwrap());
    assert_eq!(t.matches("Persona 1:**").count(), 2, "{t}");
}

#[test]
fn test_rust_fixture_with_speakers() {
    let d = tempfile::tempdir().unwrap();
    let clock = Clock::default();
    let w = TranscriptWriter::new(
        d.path(),
        "Teams",
        dt(2026, 11, 12, 10, 0, 0),
        None,
        clock.boxed(),
    )
    .unwrap();
    w.add_voiced("Otros", 1.2, "Hola, soy María.", Some(2.5), Some(0));
    w.add("Yo", 3.0, "Encantado.", Some(3.8));
    w.add_voiced("Persona 2", 6.0, "Y yo Juan.", Some(7.0), Some(1));
    w.add_voiced("Persona 1", 9.0, "Empezamos.", Some(9.8), Some(2));
    w.relabel(labels(&[(0, "María"), (1, "Persona 1"), (2, "María")]));
    let got = std::fs::read(w.close(dt(2026, 11, 12, 10, 1, 0)).unwrap()).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transcript_rust_speakers.md");
    let want = std::fs::read(fixture).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&got),
        String::from_utf8_lossy(&want)
    );
}

#[test]
fn test_voices_lists_the_segments_with_lines() {
    let d = tempfile::tempdir().unwrap();
    let (w, clock) = make(&d, "Teams");
    w.add_voiced("Otros", 1.0, "Uno", Some(2.0), Some(3));
    w.add_voiced("Otros", 2.5, "dos", Some(3.0), Some(4)); // se une a la anterior
    w.add("Yo", 6.0, "Tres", Some(7.0));
    clock.set(20.0);
    w.flush_due();
    w.add_voiced("Otros", 9.0, "Cuatro", Some(10.0), Some(7));
    w.add_voiced("Otros", 12.0, "  ", Some(13.0), Some(8)); // sin texto: no hay línea
    assert_eq!(w.voices(), std::collections::HashSet::from([3, 4, 7]));
}

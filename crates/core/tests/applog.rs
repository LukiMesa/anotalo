//! app.log: mismo formato que la app de Python y rotación a los 2 MB con dos copias.
use anotalo_core::applog::RotatingLog;
use log::{Level, Log, Record};

fn emit(log: &RotatingLog, level: Level, target: &str, msg: &str) {
    log.log(
        &Record::builder()
            .level(level)
            .target(target)
            .args(format_args!("{msg}"))
            .build(),
    );
}

#[test]
fn test_line_format_matches_python() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("app.log");
    let log = RotatingLog::new(&p, 2_000_000, 2).unwrap();
    emit(
        &log,
        Level::Warn,
        "anotalo_core::detector",
        "Llamada detectada: Teams",
    );
    emit(&log, Level::Info, "anotalo", "Anotalo iniciado");
    log.flush();
    let text = std::fs::read_to_string(&p).unwrap();
    let re = regex::Regex::new(
        r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2},\d{3} (INFO|WARNING|ERROR|DEBUG) \S+: ",
    )
    .unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines.iter().all(|l| re.is_match(l)), "{text}");
    assert!(lines[0].contains(" WARNING anotalo_core::detector: Llamada detectada: Teams"));
}

#[test]
fn test_rotates_at_max_bytes_keeping_two_backups() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("app.log");
    let log = RotatingLog::new(&p, 1_000, 2).unwrap();
    for i in 0..200 {
        emit(
            &log,
            Level::Info,
            "anotalo",
            &format!("línea {i:03} con algo de texto"),
        );
    }
    log.flush();
    assert!(p.exists());
    assert!(d.path().join("app.log.1").exists());
    assert!(d.path().join("app.log.2").exists());
    assert!(!d.path().join("app.log.3").exists());
    assert!(std::fs::metadata(&p).unwrap().len() <= 1_000);
    let last = std::fs::read_to_string(&p).unwrap();
    assert!(last.contains("línea 199"), "lo último va al archivo actual");
}

#[test]
fn test_appends_across_restarts() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("app.log");
    {
        let log = RotatingLog::new(&p, 2_000_000, 2).unwrap();
        emit(&log, Level::Info, "anotalo", "primera");
        log.flush();
    }
    let log = RotatingLog::new(&p, 2_000_000, 2).unwrap();
    emit(&log, Level::Info, "anotalo", "segunda");
    log.flush();
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("primera") && text.contains("segunda"));
}

#[test]
fn test_only_anotalo_targets_are_written() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("app.log");
    let log = RotatingLog::new(&p, 2_000_000, 2).unwrap();
    emit(&log, Level::Info, "ureq::pool", "ruido de una dependencia");
    emit(
        &log,
        Level::Info,
        "anotalo_winaudio::capture",
        "Capturando el micrófono",
    );
    log.flush();
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(!text.contains("ruido") && text.contains("Capturando"));
}

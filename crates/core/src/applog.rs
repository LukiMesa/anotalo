//! `app.log` en la carpeta de datos, con el formato de la app de Python
//! (`%(asctime)s %(levelname)s %(name)s: %(message)s`) y rotación a los 2 MB con dos copias.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

pub const MAX_BYTES: u64 = 2_000_000;
pub const BACKUPS: usize = 2;

pub struct RotatingLog {
    path: PathBuf,
    max_bytes: u64,
    backups: usize,
    file: Mutex<Option<File>>,
    stderr: bool,
}

impl RotatingLog {
    pub fn new(path: &Path, max_bytes: u64, backups: usize) -> io::Result<RotatingLog> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(RotatingLog {
            path: path.to_path_buf(),
            max_bytes,
            backups,
            file: Mutex::new(Some(file)),
            stderr: false,
        })
    }

    fn backup(&self, n: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    /// Como RotatingFileHandler: app.log → app.log.1 → app.log.2; la más antigua se borra.
    fn rotate(&self, file: &mut Option<File>) -> io::Result<()> {
        *file = None; // cerrar antes de renombrar
        if self.backups > 0 {
            let _ = fs::remove_file(self.backup(self.backups));
            for n in (1..self.backups).rev() {
                let from = self.backup(n);
                if from.exists() {
                    fs::rename(&from, self.backup(n + 1))?;
                }
            }
            fs::rename(&self.path, self.backup(1))?;
        } else {
            fs::remove_file(&self.path)?;
        }
        *file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        Ok(())
    }

    fn write_line(&self, line: &str) -> io::Result<()> {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let size = guard
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
        if size > 0 && size + line.len() as u64 > self.max_bytes {
            self.rotate(&mut guard)?;
        }
        if guard.is_none() {
            *guard = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        guard.as_mut().unwrap().write_all(line.as_bytes())
    }
}

/// El nombre de nivel de Python.
fn level_name(l: Level) -> &'static str {
    match l {
        Level::Error => "ERROR",
        Level::Warn => "WARNING",
        Level::Info => "INFO",
        Level::Debug | Level::Trace => "DEBUG",
    }
}

pub fn format_line(level: Level, target: &str, msg: &str) -> String {
    let now = chrono::Local::now();
    format!(
        "{},{:03} {} {target}: {msg}\n",
        now.format("%Y-%m-%d %H:%M:%S"),
        now.timestamp_subsec_millis(),
        level_name(level)
    )
}

impl Log for RotatingLog {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= log::max_level() && m.target().starts_with("anotalo")
    }

    fn log(&self, r: &Record) {
        if !r.target().starts_with("anotalo") {
            return;
        }
        let line = format_line(r.level(), r.target(), &r.args().to_string());
        if self.stderr {
            eprint!("{line}");
        }
        let _ = self.write_line(&line);
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.flush();
        }
    }
}

/// `dir\app.log` como registro de toda la app. Nivel INFO, o DEBUG con `ANOTALO_LOG=debug`.
pub fn init(dir: &Path, also_stderr: bool) -> io::Result<()> {
    let mut log = RotatingLog::new(&dir.join("app.log"), MAX_BYTES, BACKUPS)?;
    log.stderr = also_stderr;
    // vive lo que vive el proceso
    log::set_logger(Box::leak(Box::new(log)))
        .map_err(|_| io::Error::other("el registro ya estaba iniciado"))?;
    let debug = std::env::var("ANOTALO_LOG").is_ok_and(|v| v == "debug");
    log::set_max_level(if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    });
    Ok(())
}

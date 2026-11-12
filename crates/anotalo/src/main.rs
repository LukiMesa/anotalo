//! Anotalo: transcriptor de llamadas y notas (reescritura en Rust de Transcriptor).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anotalo_core::models::CATALOG;
use anotalo_core::routing::PLATFORMS;
use anotalo_whisper::{DeviceKind, LoadOptions, Model, Params, devices};

mod record;
mod shell;
mod tray_app;
use record::RecordArgs;

const USAGE: &str = "Uso:
  anotalo [--minimized]            (la app, en la bandeja del sistema)
  anotalo --version
  anotalo selftest [--model <modelo>] [--wav <audio.wav>]
  anotalo record --out <carpeta> [--app <etiqueta>] [--platform <teams|zoom|…>] [--process <exe>]
                 [--mic-only] [--seconds <n>] [--model <auto|large-v3-turbo|…>]
                 [--device <auto|gpu|cpu>] [--language <es>]";

/// "Anotalo <versión>", lo que imprime `--version`.
fn version_line() -> String {
    format!("Anotalo {}", env!("CARGO_PKG_VERSION"))
}

#[derive(Debug)]
enum Command {
    /// La app de la bandeja (con o sin `--minimized`: aún no hay ventana que mostrar).
    Tray,
    Version,
    Selftest {
        model: Option<PathBuf>,
        wav: Option<PathBuf>,
    },
    Record(RecordArgs),
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None => Ok(Command::Tray),
        Some("--minimized") if args.len() == 1 => Ok(Command::Tray),
        Some("--version" | "-V") if args.len() == 1 => Ok(Command::Version),
        Some("selftest") => {
            let (mut model, mut wav) = (None, None);
            let mut rest = args[1..].iter();
            while let Some(flag) = rest.next() {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("Falta el valor de {flag}. {USAGE}"))?;
                match flag.as_str() {
                    "--model" => model = Some(PathBuf::from(value)),
                    "--wav" => wav = Some(PathBuf::from(value)),
                    other => return Err(format!("Opción desconocida: {other}. {USAGE}")),
                }
            }
            Ok(Command::Selftest { model, wav })
        }
        Some("record") => parse_record(&args[1..]).map(Command::Record),
        _ => Err(USAGE.to_string()),
    }
}

fn parse_record(args: &[String]) -> Result<RecordArgs, String> {
    let mut r = RecordArgs {
        out: PathBuf::new(),
        app: None,
        platform: None,
        process: None,
        mic_only: false,
        seconds: None,
        model: "auto".into(),
        device: "auto".into(),
        language: "es".into(),
    };
    let mut out = None;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        if flag == "--mic-only" {
            r.mic_only = true;
            continue;
        }
        let value = rest
            .next()
            .ok_or_else(|| {
                format!(
                    "Falta el valor de {flag}.
{USAGE}"
                )
            })?
            .clone();
        match flag.as_str() {
            "--out" => out = Some(PathBuf::from(value)),
            "--app" => r.app = Some(value),
            "--process" => r.process = Some(value),
            "--language" => r.language = value,
            "--platform" if PLATFORMS.iter().any(|p| p.key == value) => r.platform = Some(value),
            "--platform" => {
                let keys: Vec<&str> = PLATFORMS.iter().map(|p| p.key).collect();
                return Err(format!(
                    "Plataforma desconocida: {value} (válidas: {})",
                    keys.join(", ")
                ));
            }
            "--seconds" => {
                let n = value.parse::<u64>().ok().filter(|n| *n > 0);
                r.seconds =
                    Some(n.ok_or_else(|| {
                        format!("--seconds necesita un número de segundos: {value}")
                    })?);
            }
            "--model" if value == "auto" || CATALOG.iter().any(|m| m.setting == value) => {
                r.model = value
            }
            "--model" => {
                let names: Vec<&str> = CATALOG.iter().map(|m| m.setting).collect();
                return Err(format!(
                    "Modelo desconocido: {value} (válidos: auto, {})",
                    names.join(", ")
                ));
            }
            "--device" if matches!(value.as_str(), "auto" | "gpu" | "cpu") => r.device = value,
            "--device" => {
                return Err(format!(
                    "Dispositivo desconocido: {value} (auto, gpu o cpu)"
                ));
            }
            other => {
                return Err(format!(
                    "Opción desconocida: {other}.
{USAGE}"
                ));
            }
        }
    }
    r.out = out.ok_or_else(|| {
        format!(
            "Falta --out <carpeta>.
{USAGE}"
        )
    })?;
    Ok(r)
}

/// Mensajes de Anotalo por la consola de errores ("Yo: …", "Otros: …", avisos). Con
/// `ANOTALO_LOG=debug`, también los de depuración.
struct ConsoleLog;

impl log::Log for ConsoleLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level() && m.target().starts_with("anotalo")
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            match r.level() {
                log::Level::Error => eprintln!("Error: {}", r.args()),
                log::Level::Warn => eprintln!("Aviso: {}", r.args()),
                _ => eprintln!("{}", r.args()),
            }
        }
    }

    fn flush(&self) {}
}

fn init_log() {
    if log::set_logger(&ConsoleLog).is_ok() {
        let debug = std::env::var("ANOTALO_LOG").is_ok_and(|v| v == "debug");
        log::set_max_level(if debug {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        });
    }
}

fn kind_label(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Cpu => "cpu",
        DeviceKind::Gpu => "gpu",
        DeviceKind::IntegratedGpu => "igpu",
        DeviceKind::Other => "otro",
    }
}

/// Audio mono de 16 kHz en 16 bits, como lo espera el modelo.
fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("No se pudo abrir {}: {e}", path.display()))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != 16_000 || spec.bits_per_sample != 16 {
        return Err(format!(
            "{} debe ser WAV mono de 16 kHz y 16 bits (es de {} canales, {} Hz, {} bits)",
            path.display(),
            spec.channels,
            spec.sample_rate,
            spec.bits_per_sample
        ));
    }
    reader
        .samples::<i16>()
        .map(|s| s.map(|v| f32::from(v) / 32768.0))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("No se pudo leer {}: {e}", path.display()))
}

/// Comprobación rápida del exe (la usa la integración continua): dispositivos y, si se da un
/// modelo y un audio, una transcripción.
fn selftest(model: Option<&Path>, wav: Option<&Path>) -> Result<(), String> {
    println!("{}", version_line());
    let missing = anotalo_whisper::missing_cpu_features();
    if !missing.is_empty() {
        return Err(anotalo_whisper::WhisperError::UnsupportedCpu(missing).to_string());
    }
    let devs = devices();
    for d in &devs {
        println!("Dispositivo: {} ({})", d.description, kind_label(d.kind));
    }
    let gpu = devs
        .iter()
        .any(|d| matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu));
    println!(
        "Vulkan: {}",
        if gpu { "disponible" } else { "no disponible" }
    );

    match (model, wav) {
        (None, None) => Ok(()),
        (Some(model), Some(wav)) => {
            let pcm = read_wav(wav)?;
            let mut m = Model::load(
                model,
                &LoadOptions {
                    use_gpu: true,
                    gpu_device: 0,
                },
            )
            .map_err(|e| e.to_string())?;
            let threads = std::thread::available_parallelism()
                .map(|n| n.get().min(8))
                .unwrap_or(4) as i32;
            let start = Instant::now();
            let segments = m
                .transcribe(
                    &pcm,
                    &Params {
                        language: "auto".into(),
                        threads,
                    },
                )
                .map_err(|e| e.to_string())?;
            let text: String = segments.iter().map(|s| s.text.as_str()).collect();
            println!("Transcripción: {}", text.trim());
            println!("Tiempo: {:.2} s", start.elapsed().as_secs_f64());
            Ok(())
        }
        _ => Err("Para transcribir hacen falta --model y --wav juntos".to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Command::Tray) => tray_app::run(),
        Ok(Command::Version) => {
            println!("{}", version_line());
            ExitCode::SUCCESS
        }
        Ok(Command::Selftest { model, wav }) => match selftest(model.as_deref(), wav.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Error: {e}");
                ExitCode::from(1)
            }
        },
        Ok(Command::Record(r)) => {
            init_log();
            match record::record(&r) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("Error: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Err(usage) => {
            eprintln!("{usage}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_line_names_the_product() {
        assert_eq!(
            super::version_line(),
            format!("Anotalo {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn parses_selftest_and_version() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(matches!(
            super::parse_args(&s(&["--version"])),
            Ok(super::Command::Version)
        ));
        match super::parse_args(&s(&["selftest", "--model", "m.bin", "--wav", "a.wav"])).unwrap() {
            super::Command::Selftest { model, wav } => {
                assert_eq!(model.unwrap(), std::path::PathBuf::from("m.bin"));
                assert_eq!(wav.unwrap(), std::path::PathBuf::from("a.wav"));
            }
            _ => panic!("esperaba selftest"),
        }
        assert!(super::parse_args(&s(&["selftest", "--model"])).is_err());
        assert!(super::parse_args(&s(&["otra-cosa"])).is_err());
    }

    #[test]
    fn no_arguments_or_minimized_start_the_tray_app() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(matches!(super::parse_args(&[]), Ok(super::Command::Tray)));
        assert!(matches!(
            super::parse_args(&s(&["--minimized"])),
            Ok(super::Command::Tray)
        ));
        assert!(super::parse_args(&s(&["--minimized", "x"])).is_err());
        assert!(super::USAGE.lines().nth(1).unwrap().contains("--minimized"));
    }

    #[test]
    fn parses_record() {
        use super::{Command, RecordArgs, parse_args};
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let all = parse_args(&s(&[
            "record",
            "--out",
            "C:\\Llamadas",
            "--app",
            "Cliente",
            "--platform",
            "teams",
            "--process",
            "ms-teams.exe",
            "--mic-only",
            "--seconds",
            "25",
            "--model",
            "small",
            "--device",
            "cpu",
            "--language",
            "en",
        ]))
        .unwrap();
        let Command::Record(all) = all else {
            panic!("esperaba record")
        };
        assert_eq!(
            all,
            RecordArgs {
                out: "C:\\Llamadas".into(),
                app: Some("Cliente".into()),
                platform: Some("teams".into()),
                process: Some("ms-teams.exe".into()),
                mic_only: true,
                seconds: Some(25),
                model: "small".into(),
                device: "cpu".into(),
                language: "en".into(),
            }
        );
        let Command::Record(d) = parse_args(&s(&["record", "--out", "x"])).unwrap() else {
            panic!("esperaba record")
        };
        assert_eq!(
            d,
            RecordArgs {
                out: "x".into(),
                app: None,
                platform: None,
                process: None,
                mic_only: false,
                seconds: None,
                model: "auto".into(),
                device: "auto".into(),
                language: "es".into(),
            }
        );
        for bad in [
            &["record"][..],
            &["record", "--out"],
            &["record", "--out", "x", "--bogus"],
            &["record", "--out", "x", "--seconds", "mucho"],
            &["record", "--out", "x", "--device", "tpu"],
            &["record", "--out", "x", "--model", "gigante"],
            &["record", "--out", "x", "--platform", "myspace"],
        ] {
            assert!(parse_args(&s(bad)).is_err(), "{bad:?}");
        }
    }
}

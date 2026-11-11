//! Anotalo: transcriptor de llamadas y notas (reescritura en Rust de Transcriptor).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anotalo_whisper::{DeviceKind, LoadOptions, Model, Params, devices};

const USAGE: &str =
    "Uso: anotalo --version | anotalo selftest [--model <modelo>] [--wav <audio.wav>]";

/// "Anotalo <versión>", lo que imprime `--version`.
fn version_line() -> String {
    format!("Anotalo {}", env!("CARGO_PKG_VERSION"))
}

#[derive(Debug)]
enum Command {
    Version,
    Selftest {
        model: Option<PathBuf>,
        wav: Option<PathBuf>,
    },
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
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
        _ => Err(USAGE.to_string()),
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
}

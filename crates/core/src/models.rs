//! Modelos de whisper.cpp: catálogo, descarga verificada y reanudable, y elección del dispositivo
//! (spec §5.2).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anotalo_whisper::{Device, DeviceKind};
use sha2::{Digest, Sha256};

/// Un archivo de modelo publicado en Hugging Face, con su tamaño y SHA-256 exactos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    pub setting: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// Los modelos que se ofrecen (comprobados en Hugging Face el 2026-11-11). La última entrada, el
/// turbo en f16, solo la usa el bench si la versión de 8 bits no llega a la marca de la spec §1.
pub const CATALOG: &[ModelFile] = &[
    ModelFile {
        setting: "large-v3-turbo",
        file: "ggml-large-v3-turbo-q8_0.bin",
        size: 874_188_075,
        sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
    },
    ModelFile {
        setting: "large-v3",
        file: "ggml-large-v3-q5_0.bin",
        size: 1_081_140_203,
        sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
    },
    ModelFile {
        setting: "medium",
        file: "ggml-medium-q8_0.bin",
        size: 823_369_779,
        sha256: "42a1ffcbe4167d224232443396968db4d02d4e8e87e213d3ee2e03095dea6502",
    },
    ModelFile {
        setting: "small",
        file: "ggml-small-q8_0.bin",
        size: 264_464_607,
        sha256: "49c8fb02b65e6049d5fa6c04f81f53b867b5ec9540406812c643f177317f779f",
    },
    ModelFile {
        setting: "large-v3-turbo-f16",
        file: "ggml-large-v3-turbo.bin",
        size: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    },
];

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

/// El modelo de un ajuste. `"auto"` (y cualquier valor desconocido) es large-v3-turbo con GPU y
/// small con el procesador.
pub fn model_for(setting: &str, on_gpu: bool) -> &'static ModelFile {
    let wanted = match setting {
        "large-v3-turbo" | "large-v3" | "medium" | "small" | "large-v3-turbo-f16" => setting,
        _ if on_gpu => "large-v3-turbo",
        _ => "small",
    };
    CATALOG
        .iter()
        .find(|m| m.setting == wanted)
        .expect("el catálogo tiene todos los ajustes")
}

/// `%LOCALAPPDATA%\Anotalo\models`.
pub fn models_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Anotalo").join("models")
}

/// De dónde se descargan los bytes (en los tests, de memoria).
pub trait Fetch {
    /// Bytes de `url` desde `from_byte` (0 = desde el principio).
    fn open(&self, url: &str, from_byte: u64) -> io::Result<Box<dyn Read>>;
}

/// Descarga por HTTPS con el TLS de Windows y sus certificados raíz.
pub struct HttpFetch {
    agent: ureq::Agent,
}

impl Default for HttpFetch {
    fn default() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .user_agent(concat!("Anotalo/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        HttpFetch { agent }
    }
}

impl Fetch for HttpFetch {
    fn open(&self, url: &str, from_byte: u64) -> io::Result<Box<dyn Read>> {
        let mut req = self.agent.get(url);
        if from_byte > 0 {
            req = req.header("Range", format!("bytes={from_byte}-"));
        }
        let resp = req.call().map_err(io::Error::other)?;
        if from_byte > 0 && resp.status().as_u16() != 206 {
            // El servidor no admite continuar: quien llama vuelve a empezar desde cero.
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "el servidor no admite reanudar",
            ));
        }
        Ok(Box::new(resp.into_body().into_reader()))
    }
}

/// El modelo en `dir`, descargándolo si hace falta. Un archivo completo ya presente se acepta tal
/// cual (no se vuelve a calcular su huella); si no, se descarga a `<archivo>.part` continuando lo que
/// hubiera, se comprueban tamaño y SHA-256 y se renombra. Si no coincide, se borra.
pub fn ensure_model(
    m: &ModelFile,
    dir: &Path,
    fetch: &dyn Fetch,
    progress: &mut dyn FnMut(f32),
) -> Result<PathBuf, String> {
    let path = dir.join(m.file);
    if fs::metadata(&path).is_ok_and(|md| md.len() == m.size) {
        progress(1.0);
        return Ok(path);
    }
    fs::create_dir_all(dir).map_err(|e| format!("no se pudo crear {}: {e}", dir.display()))?;
    let part = dir.join(format!("{}.part", m.file));
    let url = format!("{BASE_URL}{}", m.file);

    let mut result = download(m, &part, &url, fetch, progress);
    if matches!(&result, Err(DownloadError::CannotResume)) {
        let _ = fs::remove_file(&part);
        result = download(m, &part, &url, fetch, progress);
    }
    match result {
        Ok(()) => {}
        Err(DownloadError::Mismatch) => {
            let _ = fs::remove_file(&part);
            return Err(format!(
                "el modelo descargado no coincide con el publicado ({}); se borró, vuelve a intentarlo",
                m.file
            ));
        }
        Err(DownloadError::CannotResume) => {
            return Err("no se pudo reanudar la descarga del modelo".into());
        }
        Err(DownloadError::Io(e)) => {
            return Err(format!("no se pudo descargar el modelo {}: {e}", m.file));
        }
    }
    fs::rename(&part, &path).map_err(|e| format!("no se pudo guardar el modelo: {e}"))?;
    progress(1.0);
    Ok(path)
}

enum DownloadError {
    Mismatch,
    CannotResume,
    Io(io::Error),
}

fn download(
    m: &ModelFile,
    part: &Path,
    url: &str,
    fetch: &dyn Fetch,
    progress: &mut dyn FnMut(f32),
) -> Result<(), DownloadError> {
    let mut have = fs::metadata(part).map(|md| md.len()).unwrap_or(0);
    if have > m.size {
        fs::remove_file(part).map_err(DownloadError::Io)?;
        have = 0;
    }
    // La huella cubre todo el archivo: primero lo que ya había.
    let mut hasher = Sha256::new();
    if have > 0 {
        let mut f = File::open(part).map_err(DownloadError::Io)?;
        io::copy(&mut f, &mut hasher).map_err(DownloadError::Io)?;
    }
    if have < m.size {
        let mut reader = match fetch.open(url, have) {
            Ok(r) => r,
            Err(e) if have > 0 && e.kind() == io::ErrorKind::Unsupported => {
                return Err(DownloadError::CannotResume);
            }
            Err(e) => return Err(DownloadError::Io(e)),
        };
        let mut out = OpenOptions::new()
            .create(true)
            .append(true)
            .open(part)
            .map_err(DownloadError::Io)?;
        let mut buf = vec![0u8; 1 << 16];
        let mut last_pct = -1i32;
        loop {
            let n = reader.read(&mut buf).map_err(DownloadError::Io)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(DownloadError::Io)?;
            hasher.update(&buf[..n]);
            have += n as u64;
            let p = (have as f64 / m.size as f64).min(1.0) as f32;
            if (p * 100.0) as i32 != last_pct {
                last_pct = (p * 100.0) as i32;
                progress(p);
            }
        }
        out.sync_all().map_err(DownloadError::Io)?;
    }
    let sha: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if have != m.size || sha != m.sha256 {
        return Err(DownloadError::Mismatch);
    }
    Ok(())
}

/// Dónde corre el modelo. `device` es el índice entre las GPU dedicadas e integradas juntas, que es
/// como las numera whisper.cpp (`gpu_device`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Gpu { device: i32 },
    IntegratedGpu { device: i32 },
    Cpu,
}

/// Elige el dispositivo (spec §5.2): `"cpu"` → procesador; si no, la primera GPU dedicada; si no,
/// una integrada solo si ya se comprobó que es más rápida que el procesador; si no, el procesador.
/// El antiguo `"cuda"` cuenta como `"auto"`.
pub fn choose(
    device_setting: &str,
    devices: &[Device],
    igpu_faster_than_cpu: Option<bool>,
) -> Placement {
    if device_setting == "cpu" {
        return Placement::Cpu;
    }
    let gpus: Vec<&Device> = devices
        .iter()
        .filter(|d| matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu))
        .collect();
    if let Some(i) = gpus.iter().position(|d| d.kind == DeviceKind::Gpu) {
        return Placement::Gpu { device: i as i32 };
    }
    if igpu_faster_than_cpu == Some(true)
        && let Some(i) = gpus
            .iter()
            .position(|d| d.kind == DeviceKind::IntegratedGpu)
    {
        return Placement::IntegratedGpu { device: i as i32 };
    }
    Placement::Cpu
}

/// Hilos del procesador para whisper.cpp: todos los núcleos hasta 8. En el bench del M1 (i7-9700,
/// 8 núcleos, small q8_0 con OpenMP), 8 hilos fue un 20 % más rápido que 4.
pub fn cpu_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4) as i32
}

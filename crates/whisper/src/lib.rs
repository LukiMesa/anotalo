//! whisper.cpp 1.9.5 compilado desde el código fuente (Vulkan + CPU, estático) para Anotalo.
//!
//! El puente en C (`shim/`) concentra la API de whisper.cpp; aquí solo hay una capa segura.

use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_void};
use std::fmt;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::OnceLock;

unsafe extern "C" {
    fn anotalo_init(model_path: *const c_char, use_gpu: c_int, gpu_device: c_int) -> *mut c_void;
    fn anotalo_transcribe(
        ctx: *mut c_void,
        pcm: *const f32,
        n_samples: c_int,
        language: *const c_char,
        n_threads: c_int,
        cb: extern "C" fn(*mut c_void, *const c_char, f32, f32),
        user: *mut c_void,
    ) -> c_int;
    fn anotalo_free(ctx: *mut c_void);
    fn anotalo_device_count() -> c_int;
    fn anotalo_device_name(i: c_int) -> *const c_char;
    fn anotalo_device_description(i: c_int) -> *const c_char;
    fn anotalo_device_type(i: c_int) -> c_int;
}

/// Tipo de dispositivo de cálculo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Cpu,
    Gpu,
    IntegratedGpu,
    Other,
}

/// Un dispositivo donde puede correr el modelo.
#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    pub description: String,
    pub kind: DeviceKind,
}

/// Opciones al cargar un modelo.
#[derive(Debug, Clone)]
pub struct LoadOptions {
    pub use_gpu: bool,
    pub gpu_device: i32,
}

/// Opciones de cada transcripción.
#[derive(Debug, Clone)]
pub struct Params {
    pub language: String,
    pub threads: i32,
}

/// Un segmento transcrito, con lo que necesita el filtro de alucinaciones.
#[derive(Debug, Clone)]
pub struct Segment {
    pub text: String,
    pub avg_logprob: f32,
    pub no_speech_prob: f32,
}

#[derive(Debug)]
pub enum WhisperError {
    Load(String),
    Transcribe(i32),
    /// Al procesador le faltan instrucciones con las que está compilado el motor.
    UnsupportedCpu(Vec<&'static str>),
    /// Falta una DLL que acompaña al exe (vcomp140.dll).
    MissingRuntime(String),
}

impl fmt::Display for WhisperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WhisperError::Load(path) => write!(f, "no se pudo cargar el modelo {path}"),
            WhisperError::Transcribe(code) => write!(f, "la transcripción falló (código {code})"),
            WhisperError::UnsupportedCpu(missing) => write!(
                f,
                "este procesador no tiene las instrucciones que necesita el motor de transcripción \
                 ({}): hace falta uno con AVX2 (Intel desde 2013, AMD desde 2015)",
                missing.join(", ")
            ),
            WhisperError::MissingRuntime(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for WhisperError {}

fn c_text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: ggml devuelve cadenas terminadas en NUL que viven lo que el proceso.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Instrucciones con las que se compila whisper.cpp (base AVX2: Intel desde 2013, AMD desde 2015).
/// Sin ellas, la primera llamada a ggml cerraría el programa ("instrucción ilegal").
pub const REQUIRED_CPU_FEATURES: &[&str] = &["avx", "avx2", "fma", "f16c", "bmi2"];

fn missing_from(has: impl Fn(&str) -> bool) -> Vec<&'static str> {
    REQUIRED_CPU_FEATURES
        .iter()
        .copied()
        .filter(|f| !has(f))
        .collect()
}

/// Instrucciones que le faltan a este procesador (vacío: el motor puede funcionar).
pub fn missing_cpu_features() -> Vec<&'static str> {
    missing_from(|f| match f {
        "avx" => std::arch::is_x86_feature_detected!("avx"),
        "avx2" => std::arch::is_x86_feature_detected!("avx2"),
        "fma" => std::arch::is_x86_feature_detected!("fma"),
        "f16c" => std::arch::is_x86_feature_detected!("f16c"),
        "bmi2" => std::arch::is_x86_feature_detected!("bmi2"),
        _ => false,
    })
}

/// Dispositivos disponibles (sin Vulkan, solo la CPU).
pub fn devices() -> Vec<Device> {
    if !missing_cpu_features().is_empty() {
        return Vec::new(); // sin tocar ggml: en este procesador se cerraría
    }
    // SAFETY: funciones sin estado de ggml; índices dentro de anotalo_device_count().
    unsafe {
        (0..anotalo_device_count())
            .map(|i| Device {
                name: c_text(anotalo_device_name(i)),
                description: c_text(anotalo_device_description(i)),
                kind: match anotalo_device_type(i) {
                    0 => DeviceKind::Cpu,
                    1 => DeviceKind::Gpu,
                    2 => DeviceKind::IntegratedGpu,
                    _ => DeviceKind::Other,
                },
            })
            .collect()
    }
}

/// Un modelo cargado. Se usa desde un hilo a la vez.
pub struct Model {
    ctx: NonNull<c_void>,
}

// SAFETY: el contexto de whisper.cpp puede moverse entre hilos si no se usa a la vez (&mut self).
unsafe impl Send for Model {}

extern "C" fn on_segment(
    user: *mut c_void,
    text: *const c_char,
    avg_logprob: f32,
    no_speech_prob: f32,
) {
    // SAFETY: `user` es el &mut Vec<Segment> de transcribe(), vivo durante la llamada.
    let out = unsafe { &mut *(user as *mut Vec<Segment>) };
    out.push(Segment {
        text: c_text(text),
        avg_logprob,
        no_speech_prob,
    });
}

impl Model {
    pub fn load(path: &Path, opts: &LoadOptions) -> Result<Model, WhisperError> {
        let missing = missing_cpu_features();
        if !missing.is_empty() {
            return Err(WhisperError::UnsupportedCpu(missing));
        }
        openmp_runtime()?;
        let shown = path.display().to_string();
        let c_path = path
            .to_str()
            .and_then(|s| CString::new(s).ok())
            .ok_or_else(|| WhisperError::Load(shown.clone()))?;
        // SAFETY: ruta UTF-8 terminada en NUL; el shim devuelve NULL si falla.
        let ctx = unsafe { anotalo_init(c_path.as_ptr(), opts.use_gpu as c_int, opts.gpu_device) };
        NonNull::new(ctx)
            .map(|ctx| Model { ctx })
            .ok_or(WhisperError::Load(shown))
    }

    pub fn transcribe(
        &mut self,
        pcm_16k_mono: &[f32],
        p: &Params,
    ) -> Result<Vec<Segment>, WhisperError> {
        let language =
            CString::new(p.language.as_str()).map_err(|_| WhisperError::Transcribe(-1))?;
        let n = c_int::try_from(pcm_16k_mono.len()).map_err(|_| WhisperError::Transcribe(-2))?;
        let mut out: Vec<Segment> = Vec::new();
        // SAFETY: contexto vivo (&mut self), audio y cadenas válidos durante la llamada.
        let rc = unsafe {
            anotalo_transcribe(
                self.ctx.as_ptr(),
                pcm_16k_mono.as_ptr(),
                n,
                language.as_ptr(),
                p.threads,
                on_segment,
                &mut out as *mut Vec<Segment> as *mut c_void,
            )
        };
        if rc != 0 {
            return Err(WhisperError::Transcribe(rc));
        }
        Ok(out)
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        // SAFETY: contexto creado por anotalo_init y liberado una sola vez.
        unsafe { anotalo_free(self.ctx.as_ptr()) }
    }
}

/// Núcleos físicos del procesador (sin contar los hilos de hyper-threading), o None si Windows no
/// lo dice. El motor usa un hilo por núcleo físico (spec §5.2).
pub fn physical_cores() -> Option<usize> {
    unsafe extern "system" {
        fn GetLogicalProcessorInformationEx(relation: u32, buffer: *mut u8, len: *mut u32) -> i32;
    }
    const RELATION_PROCESSOR_CORE: u32 = 0;
    let mut len = 0u32;
    // SAFETY: con búfer nulo solo devuelve el tamaño necesario.
    unsafe {
        GetLogicalProcessorInformationEx(RELATION_PROCESSOR_CORE, std::ptr::null_mut(), &mut len)
    };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: búfer del tamaño que pidió Windows.
    if unsafe {
        GetLogicalProcessorInformationEx(RELATION_PROCESSOR_CORE, buf.as_mut_ptr(), &mut len)
    } == 0
    {
        return None;
    }
    // Un registro por núcleo; cada uno empieza por (Relationship: u32, Size: u32).
    let (mut cores, mut at) = (0, 0usize);
    while at + 8 <= len as usize {
        let size = u32::from_le_bytes(buf[at + 4..at + 8].try_into().unwrap()) as usize;
        if size == 0 {
            break;
        }
        cores += 1;
        at += size;
    }
    (cores > 0).then_some(cores)
}

/// El motor usa el OpenMP de Microsoft (vcomp140.dll, firmada por Microsoft), que va junto al exe y
/// es de carga diferida (.cargo/config.toml): sin ella, el primer cálculo cerraría el programa. Se
/// carga aquí, antes de que ggml la necesite: la de la carpeta del exe y, si no está, la de
/// System32. Nunca la que Windows encontraría buscando por su cuenta.
fn openmp_runtime() -> Result<(), WhisperError> {
    static LOADED: OnceLock<Result<(), String>> = OnceLock::new();
    LOADED
        .get_or_init(load_openmp)
        .clone()
        .map_err(WhisperError::MissingRuntime)
}

fn load_openmp() -> Result<(), String> {
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    }
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;
    let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let next_to_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("vcomp140.dll")))
        .filter(|p| p.exists());
    if let Some(local) = next_to_exe {
        let name = wide(local.as_os_str());
        // SAFETY: ruta completa terminada en NUL; el módulo queda cargado mientras viva el proceso.
        if !unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), 0) }.is_null() {
            return Ok(());
        }
    }
    let name = wide(OsStr::new("vcomp140.dll"));
    // SAFETY: nombre terminado en NUL; solo se busca en System32.
    if !unsafe {
        LoadLibraryExW(
            name.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .is_null()
    {
        return Ok(());
    }
    Err("falta vcomp140.dll junto a Anotalo.exe; reinstala Anotalo".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_the_cpu_features_whisper_needs() {
        assert!(super::missing_from(|_| true).is_empty());
        assert_eq!(super::missing_from(|f| f != "avx2"), vec!["avx2"]);
        assert_eq!(
            super::missing_from(|_| false).len(),
            super::REQUIRED_CPU_FEATURES.len()
        );
        // este PC (i7-9700) los tiene todos
        assert!(super::missing_cpu_features().is_empty());
    }
}

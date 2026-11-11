//! whisper.cpp 1.9.5 compilado desde el código fuente (Vulkan + CPU, estático) para Anotalo.
//!
//! El puente en C (`shim/`) concentra la API de whisper.cpp; aquí solo hay una capa segura.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fmt;
use std::path::Path;
use std::ptr::NonNull;

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
}

impl fmt::Display for WhisperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WhisperError::Load(path) => write!(f, "no se pudo cargar el modelo {path}"),
            WhisperError::Transcribe(code) => write!(f, "la transcripción falló (código {code})"),
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

/// Dispositivos disponibles (sin Vulkan, solo la CPU).
pub fn devices() -> Vec<Device> {
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

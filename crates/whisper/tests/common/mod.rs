//! Utilidades compartidas por los tests de integración (no es un test).
#![allow(dead_code)]

use std::path::PathBuf;

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

pub fn model() -> PathBuf {
    let p = std::env::var("ANOTALO_TEST_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root().join("target").join("models").join("ggml-tiny.bin"));
    assert!(
        p.exists(),
        "falta el modelo de prueba: ejecuta scripts/get-test-model.ps1 ({})",
        p.display()
    );
    p
}

pub fn jfk() -> Vec<f32> {
    let mut r =
        hound::WavReader::open(root().join("third_party/whisper.cpp/samples/jfk.wav")).unwrap();
    r.samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect()
}

/// Ruta de un módulo ya cargado en este proceso (p. ej. "vulkan-1.dll"), o None.
pub fn loaded_module_path(name: &str) -> Option<String> {
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
        fn GetModuleFileNameW(module: *mut std::ffi::c_void, buf: *mut u16, size: u32) -> u32;
    }
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: cadena terminada en NUL y búfer del tamaño indicado.
    unsafe {
        let module = GetModuleHandleW(wide.as_ptr());
        if module.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize;
        Some(String::from_utf16_lossy(&buf[..n]))
    }
}

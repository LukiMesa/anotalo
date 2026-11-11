//! Un error dentro de whisper.cpp (excepción de C++) llega como Err, no cierra el programa (revisión final, I3).
use anotalo_whisper::{LoadOptions, Model, WhisperError};

const CPU: LoadOptions = LoadOptions {
    use_gpu: false,
    gpu_device: 0,
};

#[test]
fn a_corrupt_model_is_an_error_not_a_crash() {
    // Cabecera válida (magia "ggml", tiny, f16) y luego n_mel = -1: whisper.cpp hace
    // vector::resize(n_mel * n_fft) y lanza std::length_error (p. ej. una descarga corrupta).
    let mut bytes = 0x6767_6d6c_u32.to_le_bytes().to_vec();
    for v in [51865_i32, 1500, 384, 6, 4, 448, 384, 6, 4, 80, 1, -1, 1] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let p = std::env::temp_dir().join("anotalo-modelo-corrupto.bin");
    std::fs::write(&p, &bytes).unwrap();
    let r = Model::load(&p, &CPU);
    let _ = std::fs::remove_file(&p);
    assert!(matches!(r, Err(WhisperError::Load(_))), "{:?}", r.err());
}

#[test]
fn a_path_whisper_cannot_open_is_an_error() {
    let p = std::env::temp_dir().join("anotalo 😀").join("modelo.bin");
    assert!(matches!(Model::load(&p, &CPU), Err(WhisperError::Load(_))));
}

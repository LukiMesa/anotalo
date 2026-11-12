//! El motor usa OpenMP (vcomp140.dll, de Microsoft, va junto al exe): se carga de la carpeta del exe
//! (o de System32), nunca de donde la busque Windows por su cuenta, y la transcripción en la CPU
//! funciona con ella (decisión del dueño, 2026-11-12: la CPU era un 20–50 % más lenta sin OpenMP).
mod common;

use anotalo_whisper::{LoadOptions, Model, Params};

#[test]
fn transcribes_on_cpu_with_the_openmp_runtime_next_to_the_exe() {
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(
        exe_dir.join("vcomp140.dll").exists(),
        "la compilación debe copiar vcomp140.dll junto a los exe ({})",
        exe_dir.display()
    );
    let mut m = Model::load(
        &common::model(),
        &LoadOptions {
            use_gpu: false,
            gpu_device: 0,
        },
    )
    .unwrap();
    let segs = m
        .transcribe(
            &common::jfk(),
            &Params {
                language: "en".into(),
                threads: 4,
            },
        )
        .unwrap();
    let text: String = segs.iter().map(|s| s.text.as_str()).collect();
    assert!(text.to_lowercase().contains("ask not"), "{text}");
    let loaded = common::loaded_module_path("vcomp140.dll").expect("vcomp140.dll sin cargar");
    let loaded = std::path::Path::new(&loaded)
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_lowercase();
    assert_eq!(loaded, exe_dir.to_string_lossy().to_lowercase());
}

mod common;

use anotalo_whisper::{DeviceKind, LoadOptions, Model, Params, devices};
use common::{jfk, model};

#[test]
fn transcribes_jfk_on_the_default_device() {
    let mut m = Model::load(
        &model(),
        &LoadOptions {
            use_gpu: true,
            gpu_device: 0,
        },
    )
    .unwrap();
    let segs = m
        .transcribe(
            &jfk(),
            &Params {
                language: "en".into(),
                threads: 4,
            },
        )
        .unwrap();
    let text: String = segs
        .iter()
        .map(|s| s.text.as_str())
        .collect::<String>()
        .to_lowercase();
    assert!(
        text.contains("ask not what your country can do for you"),
        "{text}"
    );
    assert!(
        segs.iter()
            .all(|s| s.avg_logprob <= 0.0 && (0.0..=1.0).contains(&s.no_speech_prob))
    );
}

#[test]
fn lists_a_cpu_device() {
    let d = devices();
    for x in &d {
        println!("device: {} | {} | {:?}", x.name, x.description, x.kind);
    }
    assert!(d.iter().any(|x| matches!(x.kind, DeviceKind::Cpu)));
}

#[test]
fn loads_a_model_from_a_folder_with_accents() {
    // Usuarios como C:\Users\José: la ruta del modelo lleva caracteres no ASCII.
    let dir = std::env::temp_dir().join("anotalo prueba ñandú");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("modelo.bin");
    std::fs::copy(model(), &p).unwrap();
    let loaded = Model::load(
        &p,
        &LoadOptions {
            use_gpu: false,
            gpu_device: 0,
        },
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(loaded.is_ok(), "{:?}", loaded.err());
}

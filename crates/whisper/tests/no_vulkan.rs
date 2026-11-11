mod common;

use anotalo_whisper::{DeviceKind, LoadOptions, Model, Params, devices};
use common::{jfk, model};

#[test]
fn without_the_vulkan_loader_it_runs_on_the_cpu() {
    // SAFETY: único test de este binario; se fija antes de cualquier llamada a ggml.
    unsafe { std::env::set_var("ANOTALO_NO_VULKAN", "1") };
    assert!(
        devices()
            .iter()
            .all(|d| !matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu))
    );
    let mut m = Model::load(
        &model(),
        &LoadOptions {
            use_gpu: true,
            gpu_device: 0,
        },
    )
    .unwrap();
    let text: String = m
        .transcribe(
            &jfk(),
            &Params {
                language: "en".into(),
                threads: 4,
            },
        )
        .unwrap()
        .iter()
        .map(|s| s.text.as_str())
        .collect::<String>()
        .to_lowercase();
    assert!(text.contains("ask not what your country"), "{text}");
}

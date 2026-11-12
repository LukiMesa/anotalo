//! Catálogo de modelos, descarga verificada y reanudable, y elección del dispositivo (spec §5.2).
use std::cell::RefCell;
use std::io::{self, Cursor, Read};

use anotalo_core::models::{CATALOG, Fetch, ModelFile, Placement, choose, ensure_model, model_for};
use anotalo_whisper::{Device, DeviceKind};
use sha2::{Digest, Sha256};

/// Descarga falsa desde memoria; anota desde qué byte se pidió.
struct FakeFetch {
    data: Vec<u8>,
    calls: RefCell<Vec<u64>>,
}

impl Fetch for FakeFetch {
    fn open(&self, _url: &str, from_byte: u64) -> io::Result<Box<dyn Read>> {
        self.calls.borrow_mut().push(from_byte);
        Ok(Box::new(Cursor::new(
            self.data[from_byte as usize..].to_vec(),
        )))
    }
}

fn payload() -> Vec<u8> {
    (0..200_000u32).flat_map(|i| i.to_le_bytes()).collect()
}

fn fake_model(data: &[u8]) -> ModelFile {
    let sha: String = Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ModelFile {
        setting: "prueba",
        file: "ggml-prueba.bin",
        size: data.len() as u64,
        sha256: Box::leak(sha.into_boxed_str()),
    }
}

fn fetch(data: Vec<u8>) -> FakeFetch {
    FakeFetch {
        data,
        calls: RefCell::new(Vec::new()),
    }
}

fn dev(name: &str, kind: DeviceKind) -> Device {
    Device {
        name: name.into(),
        description: name.into(),
        kind,
    }
}

#[test]
fn catalog_maps_settings() {
    assert_eq!(
        model_for("large-v3-turbo", true).file,
        "ggml-large-v3-turbo-q8_0.bin"
    );
    assert_eq!(model_for("large-v3", true).file, "ggml-large-v3-q5_0.bin");
    assert_eq!(model_for("medium", false).file, "ggml-medium-q8_0.bin");
    assert_eq!(model_for("small", true).file, "ggml-small-q8_0.bin");
    let turbo = CATALOG
        .iter()
        .find(|m| m.file == "ggml-large-v3-turbo-q8_0.bin")
        .unwrap();
    assert_eq!(turbo.size, 874_188_075);
    assert_eq!(
        turbo.sha256,
        "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1"
    );
}

#[test]
fn auto_uses_turbo_on_gpu_and_small_on_cpu() {
    assert_eq!(model_for("auto", true).setting, "large-v3-turbo");
    assert_eq!(model_for("auto", false).setting, "small");
    assert_eq!(model_for("algo-raro", false).setting, "small");
}

#[test]
fn downloads_and_verifies() {
    let d = tempfile::tempdir().unwrap();
    let data = payload();
    let m = fake_model(&data);
    let f = fetch(data.clone());
    let mut seen = Vec::new();
    let path = ensure_model(&m, d.path(), &f, &mut |p| seen.push(p)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert_eq!(*f.calls.borrow(), vec![0]);
    assert_eq!(seen.last().copied(), Some(1.0));
    assert!(!d.path().join("ggml-prueba.bin.part").exists());
}

#[test]
fn resumes_a_partial_download() {
    let d = tempfile::tempdir().unwrap();
    let data = payload();
    let m = fake_model(&data);
    let half = data.len() / 2;
    std::fs::write(d.path().join("ggml-prueba.bin.part"), &data[..half]).unwrap();
    let f = fetch(data.clone());
    let path = ensure_model(&m, d.path(), &f, &mut |_| {}).unwrap();
    assert_eq!(*f.calls.borrow(), vec![half as u64]);
    assert_eq!(std::fs::read(path).unwrap(), data);
}

#[test]
fn rejects_a_wrong_hash_and_deletes_part() {
    let d = tempfile::tempdir().unwrap();
    let data = payload();
    let mut m = fake_model(&data);
    m.sha256 = "00";
    let err = ensure_model(&m, d.path(), &fetch(data), &mut |_| {}).unwrap_err();
    assert!(err.contains("no coincide"), "{err}");
    assert!(!d.path().join("ggml-prueba.bin.part").exists());
    assert!(!d.path().join("ggml-prueba.bin").exists());
}

#[test]
fn accepts_an_existing_complete_file_without_fetching() {
    let d = tempfile::tempdir().unwrap();
    let data = payload();
    let m = fake_model(&data);
    std::fs::write(d.path().join("ggml-prueba.bin"), &data).unwrap();
    let f = fetch(data);
    ensure_model(&m, d.path(), &f, &mut |_| {}).unwrap();
    assert!(f.calls.borrow().is_empty());
}

#[test]
fn works_in_a_folder_with_accents() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("José").join("Anotalo").join("models");
    let data = payload();
    let path = ensure_model(&fake_model(&data), &dir, &fetch(data.clone()), &mut |_| {}).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), data);
}

#[test]
fn choose_prefers_discrete_gpu() {
    let devices = [
        dev("CPU", DeviceKind::Cpu),
        dev("Intel UHD", DeviceKind::IntegratedGpu),
        dev("RTX 3080", DeviceKind::Gpu),
    ];
    // whisper.cpp numera juntas las GPU dedicadas e integradas: la RTX es la 1
    assert_eq!(choose("auto", &devices, None), Placement::Gpu { device: 1 });
    assert_eq!(
        choose("gpu", &devices, Some(false)),
        Placement::Gpu { device: 1 }
    );
}

#[test]
fn choose_igpu_only_when_faster() {
    let devices = [
        dev("CPU", DeviceKind::Cpu),
        dev("Intel UHD", DeviceKind::IntegratedGpu),
    ];
    assert_eq!(
        choose("auto", &devices, Some(true)),
        Placement::IntegratedGpu { device: 0 }
    );
    assert_eq!(choose("auto", &devices, Some(false)), Placement::Cpu);
    assert_eq!(choose("auto", &devices, None), Placement::Cpu);
}

#[test]
fn choose_cpu_setting_and_no_devices() {
    let devices = [
        dev("CPU", DeviceKind::Cpu),
        dev("RTX 3080", DeviceKind::Gpu),
    ];
    assert_eq!(choose("cpu", &devices, None), Placement::Cpu);
    assert_eq!(choose("auto", &[], None), Placement::Cpu);
}

#[test]
fn legacy_cuda_setting_is_auto() {
    let devices = [dev("RTX 3080", DeviceKind::Gpu)];
    assert_eq!(choose("cuda", &devices, None), Placement::Gpu { device: 0 });
}

/// Red real (Hugging Face): TLS de Windows, redirecciones y continuar con Range.
/// `cargo test --release -p anotalo-core --test models -- --ignored`
#[test]
#[ignore]
fn http_fetch_resumes_from_hugging_face() {
    use anotalo_core::models::HttpFetch;
    let small = model_for("small", false);
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        small.file
    );
    let mut tail = Vec::new();
    HttpFetch::default()
        .open(&url, small.size - 1024)
        .unwrap()
        .read_to_end(&mut tail)
        .unwrap();
    assert_eq!(tail.len(), 1024);
}

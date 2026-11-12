//! Catálogo de modelos, descarga verificada y reanudable, y elección del dispositivo (spec §5.2).
use std::cell::RefCell;
use std::io::{self, Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anotalo_core::models::{
    CATALOG, Fetch, HttpFetch, ModelFile, Placement, TITANET, TITANET_URL, choose, ensure_download,
    ensure_model, model_for,
};
use anotalo_whisper::{Device, DeviceKind};
use sha2::{Digest, Sha256};

/// Descarga falsa desde memoria; anota desde qué byte se pidió.
struct FakeFetch {
    data: Vec<u8>,
    calls: RefCell<Vec<u64>>,
    urls: RefCell<Vec<String>>,
}

impl Fetch for FakeFetch {
    fn open(&self, url: &str, from_byte: u64) -> io::Result<Box<dyn Read>> {
        self.calls.borrow_mut().push(from_byte);
        self.urls.borrow_mut().push(url.to_string());
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
        urls: RefCell::new(Vec::new()),
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

/// Un servidor en 127.0.0.1 que atiende una conexión: lee la petición y hace `reply` con el socket.
/// Devuelve la URL.
fn local_server(reply: impl FnOnce(&mut TcpStream) + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/modelo.bin", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut req = Vec::new();
        let mut b = [0u8; 1];
        while !req.ends_with(b"\r\n\r\n") && matches!(s.read(&mut b), Ok(1)) {
            req.push(b[0]);
        }
        reply(&mut s);
    });
    url
}

/// Hasta que el cliente cuelgue (o se acabe el test): un servidor que no manda nada más.
fn hold(s: &mut TcpStream) {
    let _ = s.read(&mut [0u8; 1]);
}

/// Todo el cuerpo con conexión en 5 s y 1 s como mucho sin recibir nada. Corre en otro hilo: si se
/// queda colgado, el test falla a los 15 s en vez de no acabar nunca.
fn fetch_body(url: String) -> io::Result<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = HttpFetch::with_timeouts(Duration::from_secs(5), Duration::from_secs(1))
            .open(&url, 0)
            .and_then(|mut body| {
                let mut got = Vec::new();
                body.read_to_end(&mut got).map(|_| got)
            });
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(15))
        .expect("la descarga se quedó colgada")
}

#[test]
fn http_fetch_gives_up_when_the_server_never_answers() {
    let url = local_server(hold);
    let t = Instant::now();
    assert!(fetch_body(url).is_err());
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

#[test]
fn http_fetch_gives_up_when_the_body_stalls() {
    let url = local_server(|s| {
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nhola")
            .unwrap();
        hold(s);
    });
    let t = Instant::now();
    assert!(fetch_body(url).is_err());
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

/// La espera es sin recibir nada, no un límite a toda la descarga (los modelos tardan minutos).
#[test]
fn http_fetch_keeps_a_slow_but_steady_body() {
    let url = local_server(|s| {
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n")
            .unwrap();
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(250));
            s.write_all(b"x").unwrap();
        }
    });
    assert_eq!(fetch_body(url).unwrap(), b"xxxxxxxxxxxx");
}

#[test]
fn cpu_threads_follow_physical_cores_up_to_8() {
    // Spec §5.2: hilos = núcleos físicos (un portátil de 4 núcleos y 8 hilos usa 4), hasta 8.
    let logical = std::thread::available_parallelism().unwrap().get();
    let physical = anotalo_whisper::physical_cores().expect("núcleos físicos");
    assert!(
        physical >= 1 && physical <= logical,
        "{physical} de {logical}"
    );
    assert_eq!(
        anotalo_core::models::cpu_threads() as usize,
        physical.min(8)
    );
}

#[test]
fn test_titanet_is_pinned() {
    assert_eq!(TITANET.file, "nemo_en_titanet_small.onnx");
    assert_eq!(TITANET.size, 40_257_283);
    assert_eq!(
        TITANET.sha256,
        "ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e"
    );
    assert_eq!(
        TITANET_URL,
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx"
    );
}

#[test]
fn test_ensure_download_uses_the_given_url() {
    let d = tempfile::tempdir().unwrap();
    let data = payload();
    let m = fake_model(&data);
    let f = fetch(data);
    let p = ensure_download(
        &m,
        "https://example.invalid/x.bin",
        d.path(),
        &f,
        &mut |_| {},
    )
    .unwrap();
    assert!(p.exists());
    assert_eq!(
        f.urls.borrow().as_slice(),
        ["https://example.invalid/x.bin"]
    );
}

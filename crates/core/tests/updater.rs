//! Actualizaciones: versiones SemVer, la Release que se toma (canal estable o de pruebas), la
//! descarga comprobada y el estado para la ventana. Port de `tests/test_updater.py`; sin red: las
//! descargas salen de memoria (`FakeFetch`) y el resto son cierres en `UpdateDeps`.
use std::collections::HashMap;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anotalo_core::config::Settings;
use anotalo_core::models::Fetch;
use anotalo_core::notify::Notifier;
use anotalo_core::paths::Profile;
use anotalo_core::updater::{
    INSTALLER_ARGS, Release, UpdateDeps, UpdateManager, UpdateState, Version, announce_if_updated,
    api_url, asset_name, download, fetch_release, is_newer, pick_release, url_ok,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const URL: &str =
    "https://github.com/LukiMesa/anotalo/releases/download/v1.0.1/Anotalo-Setup-1.0.1.exe";
const OFFICIAL: &str = "La descarga no viene de la página oficial de la app";

fn payload() -> Vec<u8> {
    b"instalador de prueba".repeat(1000)
}

fn sha_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn rel(url: &str, name: &str, sha256: &str) -> Release {
    Release {
        version: "1.0.1".into(),
        title: "Anotalo 1.0.1".into(),
        url: url.into(),
        name: name.into(),
        size: payload().len() as u64,
        sha256: sha256.into(),
    }
}

fn good_rel() -> Release {
    rel(URL, "Anotalo-Setup-1.0.1.exe", &sha_hex(&payload()))
}

/// Bytes por dirección, desde memoria; anota qué se pidió.
#[derive(Default)]
struct FakeFetch {
    files: HashMap<String, Vec<u8>>,
    asked: Mutex<Vec<String>>,
}

impl FakeFetch {
    fn serving(url: &str, bytes: Vec<u8>) -> FakeFetch {
        FakeFetch {
            files: HashMap::from([(url.to_string(), bytes)]),
            ..Default::default()
        }
    }
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Fetch for FakeFetch {
    fn open(&self, url: &str, from_byte: u64) -> io::Result<Box<dyn Read>> {
        self.asked.lock().unwrap().push(url.to_string());
        assert_eq!(from_byte, 0, "las actualizaciones se descargan enteras");
        match self.files.get(url) {
            Some(b) => Ok(Box::new(Cursor::new(b.clone()))),
            None => Err(io::Error::new(io::ErrorKind::NotFound, "404")),
        }
    }
}

fn asset(v: &str) -> Value {
    json!({"name": format!("Anotalo-Setup-{v}.exe"), "size": 5, "digest": format!("sha256:{}", sha_hex(b"hello")),
           "browser_download_url": format!("https://github.com/LukiMesa/anotalo/releases/download/v{v}/Anotalo-Setup-{v}.exe")})
}

fn release(v: &str, pre: bool, draft: bool) -> Value {
    json!({"tag_name": format!("v{v}"), "name": format!("Anotalo {v}"), "prerelease": pre, "draft": draft, "assets": [asset(v)]})
}

// --- versiones -------------------------------------------------------------

#[test]
fn test_version_compare() {
    assert_eq!(Version::parse("v0.6.0"), Version::parse("0.6.0"));
    assert!(Version::parse("v0.6.0").is_some());
    assert!(is_newer("0.10.0", "0.9.0"));
    assert!(is_newer("v0.6.1", "0.6.0"));
    assert!(!is_newer("0.6.0", "0.6.0"));
    assert!(!is_newer("0.5.9", "0.6.0"));
}

#[test]
fn test_prerelease_versions_order() {
    let order = [
        "0.6.4",
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.2",
        "1.0.0-alpha.10",
        "1.0.0-beta",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1",
        "1.10.0",
    ];
    for w in order.windows(2) {
        assert!(
            is_newer(w[1], w[0]) && !is_newer(w[0], w[1]),
            "{} > {}",
            w[1],
            w[0]
        );
    }
    assert!(!is_newer("v1.0.0", "1.0.0"));
    for bad in ["1.0", "x", "1.0.0-", "01.0.0"] {
        assert!(!is_newer(bad, "0.6.4"), "{bad}");
    }
    assert!(Version::parse("1.0.0-alpha.1").unwrap().is_prerelease());
    assert!(!Version::parse("1.0.0+build.5").unwrap().is_prerelease());
    assert_eq!(Version::parse("1.0.0+build.5"), Version::parse("1.0.0"));
}

// --- qué Release se toma ---------------------------------------------------

#[test]
fn test_fetch_latest_picks_setup_asset_and_digest() {
    let data = json!({
        "tag_name": "v1.0.1", "name": "Anotalo 1.0.1", "prerelease": false, "draft": false,
        "assets": [
            {"name": "notas.txt", "browser_download_url": "https://x/notas.txt", "size": 3},
            {"name": "Anotalo-Setup-1.0.1.exe", "browser_download_url": URL, "size": 123,
             "digest": format!("sha256:{}", "AB".repeat(32))},
        ],
    });
    let f = FakeFetch::serving(api_url(false), data.to_string().into_bytes());
    let r = fetch_release(&f, false).unwrap().unwrap();
    assert_eq!(
        (
            r.version.as_str(),
            r.url.as_str(),
            r.size,
            r.sha256.as_str()
        ),
        ("1.0.1", URL, 123, "ab".repeat(32).as_str())
    );
    assert_eq!(
        (r.title.as_str(), r.name.as_str()),
        ("Anotalo 1.0.1", "Anotalo-Setup-1.0.1.exe")
    );
    assert_eq!(f.asked(), [api_url(false)]);

    let mut empty = data.clone();
    empty["assets"] = json!([]);
    let f = FakeFetch::serving(api_url(false), empty.to_string().into_bytes());
    assert_eq!(fetch_release(&f, false), Ok(None));
    // sin red, o una respuesta que no es JSON: error (la ventana lo enseña si lo pidió el usuario)
    assert!(fetch_release(&FakeFetch::default(), false).is_err());
    let f = FakeFetch::serving(api_url(false), b"<html>".to_vec());
    assert!(fetch_release(&f, false).is_err());
}

#[test]
fn test_only_the_anotalo_asset_is_taken() {
    let data = json!({"tag_name": "v1.0.1", "name": "Anotalo 1.0.1", "prerelease": false, "draft": false,
        "assets": [{"name": "Transcriptor-Setup-1.0.1.exe", "size": 1, "digest": format!("sha256:{}", "cd".repeat(32)),
                    "browser_download_url": "https://github.com/LukiMesa/anotalo/releases/download/v1.0.1/Transcriptor-Setup-1.0.1.exe"}]});
    assert!(pick_release(&data, false).is_none());
    assert!(pick_release(&data, true).is_none());
    assert_eq!(asset_name("1.0.1"), "Anotalo-Setup-1.0.1.exe");
}

#[test]
fn test_stable_build_reads_latest_only() {
    assert_eq!(
        api_url(false),
        "https://api.github.com/repos/LukiMesa/anotalo/releases/latest"
    );
    assert_eq!(
        pick_release(&release("1.0.1", false, false), false)
            .unwrap()
            .version,
        "1.0.1"
    );
    assert!(pick_release(&release("1.1.0-alpha.1", true, false), false).is_none());
    // una versión de prueba mal marcada como estable tampoco llega al canal estable
    assert!(pick_release(&release("1.1.0-alpha.1", false, false), false).is_none());
}

#[test]
fn test_prerelease_build_sees_the_newest_of_all() {
    assert_eq!(
        api_url(true),
        "https://api.github.com/repos/LukiMesa/anotalo/releases?per_page=30"
    );
    let list = json!([release("1.0.0-alpha.3", true, false), release("1.0.0-alpha.4", true, true),
                      {"tag_name": "v0.6.4", "prerelease": false, "draft": false,
                       "assets": [{"name": "Transcriptor-Setup-0.6.4.exe"}]},
                      release("1.0.0-alpha.2", true, false)]);
    assert_eq!(pick_release(&list, true).unwrap().version, "1.0.0-alpha.3");
    // la estable, cuando sale, gana a las de prueba
    let with_stable = json!([
        release("1.0.0-alpha.3", true, false),
        release("1.0.0", false, false)
    ]);
    assert_eq!(pick_release(&with_stable, true).unwrap().version, "1.0.0");
}

#[test]
fn test_missing_digest_is_refused() {
    let mut r = release("1.0.1", false, false);
    r["assets"][0]["digest"] = Value::Null;
    assert!(pick_release(&r, false).is_none());
    r["assets"][0]["digest"] = json!("sha256:1234");
    assert!(pick_release(&r, false).is_none());
}

#[test]
fn test_url_must_be_exactly_the_release_asset() {
    let n = "Anotalo-Setup-1.0.1.exe";
    assert!(url_ok(
        "https://github.com/LukiMesa/anotalo/releases/download/v1.0.1/Anotalo-Setup-1.0.1.exe",
        n,
        "1.0.1"
    ));
    for bad in [
        "https://github.com/LukiMesa/call-transcriber/releases/download/v1.0.1/Anotalo-Setup-1.0.1.exe",
        "https://github.com/LukiMesa/anotalo/releases/download/v1.0.2/Anotalo-Setup-1.0.1.exe",
        "http://github.com/LukiMesa/anotalo/releases/download/v1.0.1/Anotalo-Setup-1.0.1.exe",
        "https://github.com.evil.io/LukiMesa/anotalo/releases/download/v1.0.1/Anotalo-Setup-1.0.1.exe",
        "https://github.com/LukiMesa/anotalo/releases/download/v1.0.1/Transcriptor-Setup-1.0.1.exe",
    ] {
        assert!(!url_ok(bad, n, "1.0.1"), "{bad}");
    }
}

// --- descarga --------------------------------------------------------------

#[test]
fn test_download_verifies_size_and_hash() {
    let d = tempfile::tempdir().unwrap();
    let f = FakeFetch::serving(URL, payload());
    let mut seen = vec![];
    let path = download(&good_rel(), d.path(), &f, &mut |p| seen.push(p)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), payload());
    assert_eq!(path, d.path().join("Anotalo-Setup-1.0.1.exe"));
    assert_eq!(seen.last(), Some(&1.0));
    assert_eq!(f.asked(), [URL]);
    let names: Vec<_> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["Anotalo-Setup-1.0.1.exe"], "sin .part");
}

#[test]
fn test_download_rejects_bad_hash_and_deletes() {
    let d = tempfile::tempdir().unwrap();
    let f = FakeFetch::serving(URL, payload());
    let bad = rel(URL, "Anotalo-Setup-1.0.1.exe", &"00".repeat(32));
    assert_eq!(
        download(&bad, d.path(), &f, &mut |_| {}),
        Err("El archivo descargado no coincide con el publicado".into())
    );
    // descarga cortada
    let cut = FakeFetch::serving(URL, payload()[..100].to_vec());
    assert_eq!(
        download(&good_rel(), d.path(), &cut, &mut |_| {}),
        Err("La descarga quedó incompleta".into())
    );
    // sin red
    let err = download(&good_rel(), d.path(), &FakeFetch::default(), &mut |_| {}).unwrap_err();
    assert!(err.starts_with("No se pudo descargar: "), "{err}");
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
}

#[test]
fn test_download_rejects_foreign_url() {
    let d = tempfile::tempdir().unwrap();
    for bad in [
        "https://evil.example/Anotalo-Setup-1.0.1.exe",
        "http://github.com/LukiMesa/anotalo/releases/download/v1.0.1/Anotalo-Setup-1.0.1.exe",
        "https://github.com/otro/anotalo/releases/download/v1.0.1/Anotalo-Setup-1.0.1.exe",
        &URL.replace("Setup-1.0.1.exe", "Setup-1.0.1.exe.zip"),
    ] {
        let f = FakeFetch::serving(bad, payload());
        let r = rel(bad, "Anotalo-Setup-1.0.1.exe", &sha_hex(&payload()));
        assert_eq!(
            download(&r, d.path(), &f, &mut |_| {}),
            Err(OFFICIAL.into()),
            "{bad}"
        );
        assert!(f.asked().is_empty(), "no debe descargar {bad}");
    }
}

#[test]
fn test_download_accepts_only_the_anotalo_repo() {
    let d = tempfile::tempdir().unwrap();
    let old = "https://github.com/LukiMesa/call-transcriber/releases/download/v1.0.1/Anotalo-Setup-1.0.1.exe";
    let f = FakeFetch::serving(old, payload());
    let r = rel(old, "Anotalo-Setup-1.0.1.exe", &sha_hex(&payload()));
    assert_eq!(
        download(&r, d.path(), &f, &mut |_| {}),
        Err(OFFICIAL.into())
    );
    assert!(f.asked().is_empty());
    let f = FakeFetch::serving(URL, payload());
    let path = download(&good_rel(), d.path(), &f, &mut |_| {}).unwrap();
    assert_eq!(path.file_name().unwrap(), "Anotalo-Setup-1.0.1.exe");
}

#[test]
fn test_download_still_rejects_lookalikes() {
    let d = tempfile::tempdir().unwrap();
    for (url, name) in [
        (URL.to_string(), "Transcriptor-Setup-1.0.1.exe"), // el nombre no coincide con la URL
        (
            URL.replace("LukiMesa/anotalo", "otro/anotalo"),
            "Anotalo-Setup-1.0.1.exe",
        ),
        (
            URL.replace("/anotalo/", "/anotalo-fake/"),
            "Anotalo-Setup-1.0.1.exe",
        ),
        (
            URL.replace("Anotalo-Setup", "Anotalo-Setup-x"),
            "Anotalo-Setup-x-1.0.1.exe",
        ),
    ] {
        let f = FakeFetch::serving(&url, payload());
        let r = rel(&url, name, &sha_hex(&payload()));
        assert_eq!(
            download(&r, d.path(), &f, &mut |_| {}),
            Err(OFFICIAL.into()),
            "{url} {name}"
        );
        assert!(f.asked().is_empty(), "no debe descargar {url}");
    }
}

#[test]
fn test_clear_downloads_skips_files_in_use() {
    use std::os::windows::fs::OpenOptionsExt;
    let d = tempfile::tempdir().unwrap();
    let ours = d.path().join("Anotalo-update");
    let python = d.path().join("Transcriptor-update");
    std::fs::create_dir_all(&ours).unwrap();
    std::fs::create_dir_all(&python).unwrap();
    std::fs::write(ours.join("Anotalo-Setup-1.0.0.exe"), b"MZ").unwrap();
    std::fs::write(ours.join("Anotalo-Setup-1.0.1.exe.part"), b"MZ").unwrap();
    std::fs::write(python.join("Anotalo-Setup-1.0.0.exe"), b"MZ").unwrap();
    // el instalador que acaba de abrir esta app sigue en marcha: no se puede borrar
    let running = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ
        .open(ours.join("Anotalo-Setup-1.0.0.exe"))
        .unwrap();
    anotalo_core::updater::clear_downloads(&[ours.clone(), python.clone(), d.path().join("no")]);
    assert!(ours.join("Anotalo-Setup-1.0.0.exe").exists());
    assert!(!ours.join("Anotalo-Setup-1.0.1.exe.part").exists());
    assert!(!python.exists());
    drop(running);
    anotalo_core::updater::clear_downloads(std::slice::from_ref(&ours));
    assert!(!ours.exists());
    assert!(
        anotalo_core::updater::download_dir().ends_with("Anotalo-update")
            && anotalo_core::updater::download_dir().starts_with(std::env::temp_dir())
    );
}

// --- el estado para la ventana ---------------------------------------------

#[derive(Default)]
struct Notes(Mutex<Vec<(String, String)>>);

impl Notifier for Notes {
    fn notify(&self, title: &str, body: &str, _open: Option<&Path>) {
        self.0.lock().unwrap().push((title.into(), body.into()));
    }
}

/// Lo que ven los cierres de `UpdateDeps`, y lo que hicieron.
#[derive(Default)]
struct Shared {
    /// La versión publicada (None: la Release no tiene instalador).
    latest: Mutex<Option<String>>,
    recording: AtomicBool,
    fail_fetch: AtomicBool,
    fetches: AtomicUsize,
    /// Cuántas de las próximas descargas fallan.
    fail_downloads: AtomicUsize,
    /// La descarga hace que empiece otra grabación.
    record_on_download: AtomicBool,
    downloads: AtomicUsize,
    installed: Mutex<Vec<PathBuf>>,
    notified: Mutex<Vec<String>>,
    updating: Mutex<Vec<bool>>,
    changes: AtomicUsize,
    /// "grabando?" y "actualizando=…", en el orden en que se llamaron.
    calls: Mutex<Vec<String>>,
}

struct Harness {
    mgr: Arc<UpdateManager>,
    s: Arc<Shared>,
    notes: Arc<Notes>,
}

impl Harness {
    /// La 1.0.1 publicada (la app es la 1.0.0), sin grabar; el instalador tarda lo normal.
    fn new() -> Harness {
        Harness::build("1.0.1", "", Duration::from_secs(120))
    }

    /// Como `new`, pero la app "no se cierra" tras 50 ms.
    fn quick_timeout() -> Harness {
        Harness::build("1.0.1", "", Duration::from_millis(50))
    }

    fn build(latest: &str, notified: &str, install_timeout: Duration) -> Harness {
        let s = Arc::new(Shared::default());
        *s.latest.lock().unwrap() = (!latest.is_empty()).then(|| latest.to_string());
        let notes = Arc::new(Notes::default());
        let (s1, s2, s3, s4, s5, s6, s7) = (
            s.clone(),
            s.clone(),
            s.clone(),
            s.clone(),
            s.clone(),
            s.clone(),
            s.clone(),
        );
        let mgr = UpdateManager::new(UpdateDeps {
            current: "1.0.0".into(),
            is_recording: Box::new(move || {
                s1.calls.lock().unwrap().push("grabando?".into());
                s1.recording.load(Ordering::SeqCst)
            }),
            notifier: notes.clone(),
            on_change: Box::new(move |_| {
                s2.changes.fetch_add(1, Ordering::SeqCst);
            }),
            set_updating: Box::new(move |on| {
                s3.calls.lock().unwrap().push(format!("actualizando={on}"));
                s3.updating.lock().unwrap().push(on);
            }),
            fetch: Box::new(move || {
                s4.fetches.fetch_add(1, Ordering::SeqCst);
                if s4.fail_fetch.load(Ordering::SeqCst) {
                    return Err("sin conexión".into());
                }
                Ok(s4.latest.lock().unwrap().as_ref().map(|v| Release {
                    version: v.clone(),
                    title: format!("Anotalo {v}"),
                    url: format!(
                        "https://github.com/LukiMesa/anotalo/releases/download/v{v}/Anotalo-Setup-{v}.exe"
                    ),
                    name: format!("Anotalo-Setup-{v}.exe"),
                    size: 5,
                    sha256: sha_hex(b"hello"),
                }))
            }),
            download: Box::new(move |rel, progress| {
                s5.downloads.fetch_add(1, Ordering::SeqCst);
                if s5
                    .fail_downloads
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
                {
                    return Err("descarga dañada".into());
                }
                if s5.record_on_download.load(Ordering::SeqCst) {
                    s5.recording.store(true, Ordering::SeqCst); // empieza otra llamada mientras se descarga
                }
                progress(0.5);
                progress(1.0);
                Ok(PathBuf::from(format!("setup-{}.exe", rel.version)))
            }),
            launch: Box::new(move |p| {
                s6.installed.lock().unwrap().push(p.to_path_buf());
                Ok(())
            }),
            notified_version: notified.into(),
            on_notified: Box::new(move |v| s7.notified.lock().unwrap().push(v.to_string())),
            install_timeout,
        });
        Harness { mgr, s, notes }
    }

    fn installed(&self) -> Vec<PathBuf> {
        self.s.installed.lock().unwrap().clone()
    }
    fn updating(&self) -> Vec<bool> {
        self.s.updating.lock().unwrap().clone()
    }
    fn calls(&self) -> Vec<String> {
        self.s.calls.lock().unwrap().clone()
    }
    fn fetches(&self) -> usize {
        self.s.fetches.load(Ordering::SeqCst)
    }
    fn notes(&self) -> Vec<(String, String)> {
        self.notes.0.lock().unwrap().clone()
    }
}

fn setup_101() -> Vec<PathBuf> {
    vec![PathBuf::from("setup-1.0.1.exe")]
}

#[test]
fn test_check_background_errors_are_silent_user_errors_shown() {
    let h = Harness::new();
    h.s.fail_fetch.store(true, Ordering::SeqCst);
    h.mgr.check(false);
    assert_eq!(h.mgr.state(), UpdateState::Idle);
    assert_eq!(h.mgr.snapshot()["error"], Value::Null);
    h.mgr.check(true);
    assert_eq!(h.mgr.state(), UpdateState::Error);
    let snap = h.mgr.snapshot();
    assert_eq!(snap["state"], "error");
    assert!(
        snap["error"].as_str().unwrap().contains("sin conexión"),
        "{snap}"
    );
    h.s.fail_fetch.store(false, Ordering::SeqCst);
    h.mgr.check(true);
    assert_eq!(h.mgr.state(), UpdateState::Available);
    assert_eq!(
        h.mgr.snapshot(),
        json!({"state": "available", "version": "1.0.1", "title": "Anotalo 1.0.1",
               "progress": null, "error": null, "current": "1.0.0"})
    );
    assert!(
        h.s.changes.load(Ordering::SeqCst) >= 4,
        "cada cambio se avisa"
    );
}

#[test]
fn test_latest_version_reported_only_when_user_asks() {
    let h = Harness::build("1.0.0", "", Duration::from_secs(120));
    h.mgr.check(false);
    assert_eq!(h.mgr.state(), UpdateState::Idle);
    h.mgr.check(true);
    assert_eq!(h.mgr.state(), UpdateState::Latest);
    assert_eq!(h.mgr.snapshot()["state"], "latest");
}

#[test]
fn test_install_waits_for_recording_end_and_runs_once() {
    let h = Harness::new();
    h.s.recording.store(true, Ordering::SeqCst);
    h.mgr.check(true);
    h.mgr.install();
    h.mgr.install();
    assert_eq!(h.mgr.state(), UpdateState::Waiting);
    assert!(h.installed().is_empty());
    h.s.recording.store(false, Ordering::SeqCst);
    h.mgr.on_recording_end();
    h.mgr.on_recording_end();
    h.mgr.install();
    assert_eq!(h.installed(), setup_101());
    assert_eq!(h.mgr.state(), UpdateState::Installing);
}

#[test]
fn test_install_without_recording_runs_now() {
    let h = Harness::new();
    h.mgr.check(false);
    h.mgr.install();
    assert_eq!(h.installed(), setup_101());
    assert_eq!(h.mgr.snapshot()["progress"], json!(1.0));
}

#[test]
fn test_notifies_once_per_version() {
    let h = Harness::new();
    h.mgr.check(false);
    h.mgr.check(false);
    assert_eq!(
        h.notes(),
        [(
            "Actualización disponible".to_string(),
            "Versión 1.0.1 lista para instalar desde la app".to_string()
        )]
    );
    assert_eq!(*h.s.notified.lock().unwrap(), ["1.0.1"]);
    // ya avisado en un arranque anterior
    let h2 = Harness::build("1.0.1", "1.0.1", Duration::from_secs(120));
    h2.mgr.check(false);
    assert!(h2.notes().is_empty());
    assert_eq!(h2.mgr.state(), UpdateState::Available);
}

#[test]
fn test_download_error_is_reported_and_can_retry() {
    let h = Harness::new();
    h.s.fail_downloads.store(1, Ordering::SeqCst);
    h.mgr.check(false);
    h.mgr.install();
    assert_eq!(h.mgr.state(), UpdateState::Error);
    assert!(
        h.mgr.snapshot()["error"]
            .as_str()
            .unwrap()
            .contains("dañada")
    );
    assert!(h.installed().is_empty());
    assert_eq!(h.mgr.snapshot()["progress"], Value::Null);
    h.mgr.install();
    assert_eq!(h.installed(), setup_101());
}

#[test]
fn test_installer_args() {
    assert_eq!(
        INSTALLER_ARGS,
        ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]
    );
}

#[test]
fn test_first_run_after_update_notifies_once() {
    let mut s = Settings::defaults(Profile::Installed);
    let notes = Notes::default();
    // primera vez: guarda, sin aviso
    assert!(announce_if_updated(&mut s, "0.6.0", &notes));
    assert!(notes.0.lock().unwrap().is_empty());
    assert_eq!(s.last_run_version, "0.6.0");
    assert!(!announce_if_updated(&mut s, "0.6.0", &notes));
    assert!(announce_if_updated(&mut s, "0.6.1", &notes));
    assert_eq!(
        *notes.0.lock().unwrap(),
        [(
            "Anotalo actualizado".to_string(),
            "Ya tienes la versión 0.6.1".to_string()
        )]
    );
    assert_eq!(s.last_run_version, "0.6.1");
}

/// Quien viene de Transcriptor (la app de Python, versiones 0.x) trae su `last_run_version` en los
/// ajustes copiados: pasar a Anotalo no es "Anotalo actualizado".
#[test]
fn test_coming_from_transcriptor_is_not_an_update() {
    let mut s = Settings::defaults(Profile::Installed);
    s.last_run_version = "0.6.4".into();
    let notes = Notes::default();
    assert!(announce_if_updated(&mut s, "1.0.0-alpha.3", &notes));
    assert!(notes.0.lock().unwrap().is_empty(), "sin aviso");
    assert_eq!(s.last_run_version, "1.0.0-alpha.3", "se apunta la versión");
    // la siguiente actualización de Anotalo sí se avisa
    assert!(announce_if_updated(&mut s, "1.0.0", &notes));
    assert_eq!(
        *notes.0.lock().unwrap(),
        [(
            "Anotalo actualizado".to_string(),
            "Ya tienes la versión 1.0.0".to_string()
        )]
    );
}

/// Primero se bloquean las grabaciones y luego se mira si hay una: así ninguna puede empezar entre
/// la última comprobación y el instalador. Si la hay, se deshace y se espera.
#[test]
fn test_recordings_are_blocked_before_the_last_check() {
    let h = Harness::new();
    h.mgr.check(false);
    h.mgr.install();
    assert_eq!(h.mgr.state(), UpdateState::Installing);
    let calls = h.calls();
    assert_eq!(
        calls[calls.len() - 2..],
        ["actualizando=true", "grabando?"],
        "{calls:?}"
    );

    let h = Harness::new();
    h.s.record_on_download.store(true, Ordering::SeqCst);
    h.mgr.check(false);
    h.mgr.install();
    assert_eq!(h.mgr.state(), UpdateState::Waiting);
    let calls = h.calls();
    assert_eq!(
        calls[calls.len() - 3..],
        ["actualizando=true", "grabando?", "actualizando=false"],
        "{calls:?}"
    );
}

#[test]
fn test_recording_started_during_download_waits_and_reuses_file() {
    let h = Harness::new();
    h.s.record_on_download.store(true, Ordering::SeqCst);
    h.mgr.check(false);
    h.mgr.install();
    assert_eq!(h.mgr.state(), UpdateState::Waiting);
    assert!(h.installed().is_empty());
    assert_eq!(
        h.updating().last(),
        Some(&false),
        "esperando no bloquea las grabaciones"
    );
    h.s.record_on_download.store(false, Ordering::SeqCst);
    h.s.recording.store(false, Ordering::SeqCst);
    h.mgr.on_recording_end();
    assert_eq!(h.installed(), setup_101());
    assert_eq!(
        h.s.downloads.load(Ordering::SeqCst),
        1,
        "no vuelve a descargar"
    );
}

#[test]
fn test_installing_that_never_closes_the_app_becomes_error() {
    let h = Harness::quick_timeout();
    h.mgr.check(false);
    h.mgr.install();
    assert_eq!(h.mgr.state(), UpdateState::Installing);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(h.mgr.state(), UpdateState::Error);
    assert_eq!(
        h.mgr.snapshot()["error"],
        "La instalación no se completó. Vuelve a intentarlo."
    );
    h.mgr.install(); // se puede reintentar
    assert_eq!(
        h.installed(),
        [
            PathBuf::from("setup-1.0.1.exe"),
            PathBuf::from("setup-1.0.1.exe")
        ]
    );
    assert_eq!(h.s.downloads.load(Ordering::SeqCst), 2);
}

#[test]
fn test_installing_blocks_new_recordings() {
    let h = Harness::quick_timeout(); // la 1.0.1 publicada, se lanza bien, 50 ms de espera
    h.mgr.check(true);
    h.mgr.install();
    assert_eq!(h.mgr.state(), UpdateState::Installing);
    assert_eq!(h.updating(), [true]);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(h.mgr.state(), UpdateState::Error);
    assert_eq!(h.updating(), [true, false]);
}

#[test]
fn test_background_checks_respect_the_setting() {
    let h = Harness::new();
    let on = Arc::new(AtomicBool::new(false));
    h.mgr.start_background(
        Duration::from_millis(10),
        Duration::from_millis(10),
        on.clone(),
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(h.fetches(), 0);
    on.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    assert!(h.fetches() >= 1);
    h.mgr.stop();
    std::thread::sleep(Duration::from_millis(50));
    let after_stop = h.fetches();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(h.fetches(), after_stop, "parado, no comprueba más");
}

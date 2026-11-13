//! Lo que usan el instalador y el desinstalador: registro, accesos directos, procesos, señales y
//! WebView2. Seguros en CI: carpetas temporales y la clave de prueba `Software\AnotaloTest\<pid>-<n>`
//! (se borra al terminar); nunca las claves ni los accesos directos reales de Anotalo o Transcriptor.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anotalo_winsys::registry::{self, RegValue};
use anotalo_winsys::shortcut::{self, Folder, Shortcut};
use anotalo_winsys::{instance, process, webview2};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyW};
use windows::core::w;

/// Una clave propia bajo `HKCU\Software\AnotaloTest`, borrada en `Drop` (y la carpeta de pruebas,
/// si queda vacía).
struct TestKey(String);

impl TestKey {
    fn new() -> TestKey {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        TestKey(format!(r"Software\AnotaloTest\{}-{n}", std::process::id()))
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        let _ = registry::delete_tree(&self.0);
        // SAFETY: nombre constante; solo borra la clave si no le quedan subclaves.
        let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, w!(r"Software\AnotaloTest")) };
    }
}

#[test]
fn test_registry_roundtrip_under_a_test_key() {
    let k = TestKey::new();
    registry::set_values(
        &k.0,
        &[
            ("DisplayName", RegValue::Str("Anotalo")),
            ("NoModify", RegValue::Dword(1)),
        ],
    )
    .unwrap();
    assert_eq!(
        registry::get_string(&k.0, "DisplayName").as_deref(),
        Some("Anotalo")
    );
    assert_eq!(registry::get_dword(&k.0, "NoModify"), Some(1));
    assert!(registry::exists(&k.0));
    registry::delete_tree(&k.0).unwrap();
    assert!(!registry::exists(&k.0) && registry::delete_tree(&k.0).is_ok());
}

#[test]
fn test_shortcut_roundtrip_with_aumid() {
    let d = tempfile::tempdir().unwrap();
    let lnk = d.path().join("Inicio").join("Anotalo.lnk");
    let s = Shortcut {
        target: std::env::current_exe().unwrap(),
        args: "--minimized".into(),
        workdir: d.path().into(),
        description: "Anotalo".into(),
        aumid: Some("Anotalo.Test".into()),
    };
    shortcut::create(&lnk, &s).unwrap();
    let back = shortcut::read(&lnk).unwrap();
    assert_eq!(
        (back.target, back.args.as_str(), back.aumid.as_deref()),
        (s.target, "--minimized", Some("Anotalo.Test"))
    );
}

#[test]
fn test_known_folders() {
    let startup = shortcut::known_folder(Folder::Startup).unwrap();
    assert!(startup.is_absolute() && startup.ends_with("Startup"));
    assert!(
        shortcut::known_folder(Folder::Programs)
            .unwrap()
            .ends_with("Programs")
    );
}

#[test]
fn test_process_list_and_lifecycle() {
    let me = process::list()
        .into_iter()
        .find(|p| p.pid == std::process::id())
        .unwrap();
    assert_eq!(
        me.path.unwrap().to_string_lossy().to_lowercase(),
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .to_lowercase()
    );
    let ping = PathBuf::from(r"C:\Windows\System32\PING.EXE");
    let pid =
        process::spawn_detached(&ping, &["-n", "30", "127.0.0.1"], Path::new(r"C:\")).unwrap();
    process::terminate(pid).unwrap();
    assert!(process::wait_exit(&[pid], Duration::from_secs(5)));
}

#[test]
fn test_signals_and_held_mutexes() {
    let name = format!("AnotaloTest{}", std::process::id());
    let g = instance::acquire(&name).unwrap();
    assert!(instance::is_held(&name));
    drop(g);
    assert!(!instance::is_held(&name));
    let (tx, rx) = std::sync::mpsc::channel();
    instance::on_signal(
        &format!("{name}Quit"),
        Box::new(move || {
            let _ = tx.send(());
        }),
    );
    std::thread::sleep(Duration::from_millis(100));
    instance::signal(&format!("{name}Quit"));
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
}

#[test]
fn test_pick_webview2_version() {
    assert_eq!(
        webview2::pick_version(&[None, Some("0.0.0.0".into()), Some("".into())]),
        None
    );
    assert_eq!(
        webview2::pick_version(&[None, Some("131.0.2903.70".into())]).as_deref(),
        Some("131.0.2903.70")
    );
}

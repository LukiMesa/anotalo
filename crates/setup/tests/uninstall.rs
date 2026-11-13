//! Desinstalar Anotalo sobre un PC de mentira (`common`): nunca toca las grabaciones, y sin la
//! casilla tampoco los ajustes ni los modelos.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anotalo_setup::uninstall::{deferred_delete_args, schedule_self_delete, uninstall};
use anotalo_winsys::registry;
use common::{Env, FakeSystem, hashes};

#[test]
fn test_uninstall_removes_program_shortcuts_and_keys() {
    let e = Env::new();
    e.installed();
    fs::write(e.l.install_dir().join("Anotalo.exe.old-1"), b"x").unwrap();
    fs::write(e.l.install_dir().join("Anotalo.exe.new"), b"x").unwrap();
    let sys = FakeSystem {
        in_dir: vec![55],
        ..Default::default()
    };
    let r = uninstall(&e.l, false, &sys, &e.l.exe());
    assert_eq!(r.deferred, Some(e.l.exe()));
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert!(e.l.exe().exists(), "el exe propio se borra al salir");
    assert!(
        !e.l.install_dir().join("vcomp140.dll").exists()
            && !e.l.install_dir().join("Anotalo.exe.old-1").exists()
            && !e.l.install_dir().join("Anotalo.exe.new").exists()
    );
    for lnk in [e.l.start_menu_lnk(), e.l.startup_lnk(), e.l.desktop_lnk()] {
        assert!(!lnk.exists(), "{}", lnk.display());
    }
    assert!(!registry::exists(&e.l.uninstall_key()) && !registry::exists(&e.l.aumid_key));
    assert!(sys.calls().contains(&"terminate [55]".to_string()));
    assert!(!sys.calls().iter().any(|c| c.starts_with("launch")));
}

#[test]
fn test_uninstall_from_elsewhere_removes_the_folder() {
    let e = Env::new();
    e.installed();
    let sys = FakeSystem {
        app_running: true,
        app_quits: true,
        ..Default::default()
    };
    let r = uninstall(&e.l, false, &sys, Path::new(r"C:\otro.exe"));
    assert_eq!(r.deferred, None);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert!(!e.l.install_dir().exists());
    assert_eq!(
        sys.calls()[0],
        "quit",
        "primero se le pide a la app que se cierre"
    );
}

#[test]
fn test_uninstall_never_touches_recordings() {
    let e = Env::new();
    e.installed();
    let files = e.user_data(); // includes Documents\Llamadas\a.md and .papelera\b.md
    let recs: Vec<_> = files
        .iter()
        .filter(|p| p.to_string_lossy().contains("Llamadas"))
        .cloned()
        .collect();
    assert_eq!(recs.len(), 2);
    let before = hashes(&recs);
    uninstall(
        &e.l,
        true,
        &FakeSystem::default(),
        Path::new(r"C:\otro.exe"),
    );
    assert_eq!(hashes(&recs), before);
}

#[test]
fn test_purge_skips_a_data_dir_holding_the_recordings() {
    let e = Env::new();
    e.installed();
    let inside = e.l.data_dir().join("grabaciones");
    fs::create_dir_all(&inside).unwrap();
    fs::write(inside.join("a.md"), "x").unwrap();
    fs::write(
        e.l.data_dir().join("config.json"),
        serde_json::json!({"output_dir": inside}).to_string(),
    )
    .unwrap();
    for d in [
        e.l.local_dir(),
        e.l.legacy_data_dir(),
        e.l.legacy_local_dir(),
    ] {
        fs::create_dir_all(&d).unwrap();
    }
    let r = uninstall(
        &e.l,
        true,
        &FakeSystem::default(),
        Path::new(r"C:\otro.exe"),
    );
    assert!(inside.join("a.md").exists());
    assert!(
        r.problems
            .iter()
            .any(|p| p.contains("contiene tus grabaciones"))
    );
    for d in [
        e.l.local_dir(),
        e.l.legacy_data_dir(),
        e.l.legacy_local_dir(),
    ] {
        assert!(!d.exists(), "{}", d.display());
    }
}

#[test]
fn test_purge_keeps_the_folder_of_python_recordings() {
    let e = Env::new();
    e.installed();
    // la app de Python guardaba (por raro que sea) dentro de su carpeta de ajustes
    let inside = e.l.legacy_data_dir().join("Llamadas");
    fs::create_dir_all(&inside).unwrap();
    fs::write(inside.join("a.md"), "x").unwrap();
    fs::write(
        e.l.legacy_data_dir().join("config.json"),
        serde_json::json!({"output_dir": inside}).to_string(),
    )
    .unwrap();
    fs::create_dir_all(e.l.data_dir()).unwrap();
    let r = uninstall(
        &e.l,
        true,
        &FakeSystem::default(),
        Path::new(r"C:\otro.exe"),
    );
    assert!(inside.join("a.md").exists());
    assert_eq!(
        r.problems,
        [format!(
            "No borro {}: contiene tus grabaciones",
            e.l.legacy_data_dir().display()
        )]
    );
    assert!(!e.l.data_dir().exists());
}

#[test]
fn test_purge_reads_the_recordings_folder_before_deleting_anything() {
    let e = Env::new();
    e.installed();
    // el config.json que dice dónde están las grabaciones se borra antes que la carpeta que
    // las tiene
    let inside = e.l.legacy_local_dir().join("Llamadas");
    fs::create_dir_all(&inside).unwrap();
    fs::write(inside.join("a.md"), "x").unwrap();
    fs::create_dir_all(e.l.data_dir()).unwrap();
    fs::write(
        e.l.data_dir().join("config.json"),
        serde_json::json!({"output_dir": inside}).to_string(),
    )
    .unwrap();
    let r = uninstall(
        &e.l,
        true,
        &FakeSystem::default(),
        Path::new(r"C:\otro.exe"),
    );
    assert!(!e.l.data_dir().exists());
    assert!(inside.join("a.md").exists(), "{:?}", r.problems);
}

#[test]
fn test_silent_uninstall_keeps_settings_and_models() {
    let e = Env::new();
    e.installed();
    for d in [
        e.l.data_dir(),
        e.l.local_dir().join("models"),
        e.l.legacy_data_dir(),
    ] {
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("x"), "x").unwrap();
    }
    let update = e.l.temp.join("Anotalo-update");
    fs::create_dir_all(&update).unwrap();
    fs::write(update.join("Anotalo-Setup-9.9.9.exe"), b"MZ").unwrap();
    uninstall(
        &e.l,
        false,
        &FakeSystem::default(),
        Path::new(r"C:\otro.exe"),
    );
    for d in [
        e.l.data_dir(),
        e.l.local_dir().join("models"),
        e.l.legacy_data_dir(),
    ] {
        assert!(d.join("x").exists());
    }
    assert!(!e.l.temp.join("Anotalo-update").exists());
}

#[test]
fn test_deferred_delete_args() {
    let exe = Path::new(r"C:\Users\Ana María & Co\AppData\Local\Programs\Anotalo\Anotalo.exe");
    let dir = exe.parent().unwrap();
    assert_eq!(
        deferred_delete_args(exe, dir).unwrap(),
        r#"/d /s /c "ping -n 3 127.0.0.1 >nul & del /f /q "C:\Users\Ana María & Co\AppData\Local\Programs\Anotalo\Anotalo.exe" & rd "C:\Users\Ana María & Co\AppData\Local\Programs\Anotalo"""#
    );
    assert!(
        deferred_delete_args(Path::new(r"C:\100%\Anotalo.exe"), Path::new(r"C:\100%")).is_none()
    );
    assert!(deferred_delete_args(Path::new(r"C:\a!b\Anotalo.exe"), Path::new(r"C:\a!b")).is_none());
}

/// De verdad, con el `cmd.exe` y el `ping.exe` de Windows, sobre una carpeta temporal con un nombre
/// difícil (espacios, `&`, acentos): el exe y su carpeta desaparecen a los pocos segundos.
#[test]
fn test_self_delete_removes_the_exe_and_its_folder() {
    let d = tempfile::tempdir().unwrap();
    let dir: PathBuf = d.path().join("Ana María & Co").join("Anotalo");
    fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("Anotalo.exe");
    fs::write(&exe, b"MZ").unwrap();
    schedule_self_delete(&exe, &dir).unwrap();
    assert!(exe.exists(), "espera unos segundos antes de borrar");
    let t = Instant::now();
    while dir.exists() && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!exe.exists() && !dir.exists());
    assert!(d.path().join("Ana María & Co").exists(), "solo su carpeta");
}

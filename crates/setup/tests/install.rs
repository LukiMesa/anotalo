//! Instalar Anotalo y sustituir al Transcriptor de Inno, sobre un PC de mentira (`common`).
mod common;

use std::fs;
use std::os::windows::fs::OpenOptionsExt;

use anotalo_setup::install::{Options, Step, clean_leftovers, inspect, install};
use anotalo_winsys::registry;
use anotalo_winsys::shortcut;
use common::{Env, FILE_SHARE_DELETE, FILE_SHARE_READ, FakeSystem, hashes, old_files, payload};

#[test]
fn test_fresh_install_lays_out_everything() {
    let e = Env::new();
    let sys = FakeSystem::default();
    let exe = e.setup_exe();
    let found = inspect(&e.l, &sys);
    assert_eq!(
        found.defaults,
        Options {
            autostart: true,
            desktop: false
        }
    );
    assert!(found.anotalo.is_none() && found.inno.is_none());
    install(
        &e.l,
        &payload(&exe),
        found.defaults,
        true,
        &sys,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(e.l.exe()).unwrap(), fs::read(&exe).unwrap());
    assert_eq!(
        fs::read(e.l.install_dir().join("vcomp140.dll")).unwrap(),
        b"MZ vcomp"
    );
    let menu = shortcut::read(&e.l.start_menu_lnk()).unwrap();
    assert_eq!(
        (menu.target, menu.aumid.as_deref()),
        (e.l.exe(), Some("Anotalo"))
    );
    assert_eq!(
        shortcut::read(&e.l.startup_lnk()).unwrap().args,
        "--minimized"
    );
    assert!(!e.l.desktop_lnk().exists());
    let k = e.l.uninstall_key();
    assert_eq!(
        registry::get_string(&k, "DisplayName").as_deref(),
        Some("Anotalo")
    );
    assert_eq!(
        registry::get_string(&k, "DisplayVersion").as_deref(),
        Some("9.9.9")
    );
    assert_eq!(
        registry::get_string(&k, "Publisher").as_deref(),
        Some("LukiMesa")
    );
    assert_eq!(
        registry::get_string(&k, "UninstallString").unwrap(),
        format!("\"{}\" --uninstall", e.l.exe().display())
    );
    assert_eq!(
        registry::get_string(&k, "QuietUninstallString").unwrap(),
        format!("\"{}\" --uninstall /S", e.l.exe().display())
    );
    assert_eq!(
        registry::get_string(&k, "DisplayIcon").unwrap(),
        format!("{},0", e.l.exe().display())
    );
    assert_eq!(registry::get_dword(&k, "NoModify"), Some(1));
    assert_eq!(registry::get_dword(&k, "NoRepair"), Some(1));
    assert_eq!(registry::get_dword(&k, "EstimatedSize"), Some(1));
    assert_eq!(registry::get_string(&k, "InstallDate").unwrap().len(), 8);
    assert_eq!(
        registry::get_string(&e.l.aumid_key, "DisplayName").as_deref(),
        Some("Anotalo")
    );
    assert_eq!(
        sys.calls().last().unwrap(),
        &format!("launch {} --minimized", e.l.exe().display())
    );
}

#[test]
fn test_interactive_install_opens_the_window() {
    let (e, sys) = (Env::new(), FakeSystem::default());
    install(
        &e.l,
        &payload(&e.setup_exe()),
        Options {
            autostart: false,
            desktop: true,
        },
        false,
        &sys,
        &mut |_| {},
    )
    .unwrap();
    assert!(e.l.desktop_lnk().exists() && !e.l.startup_lnk().exists());
    assert_eq!(
        sys.calls().last().unwrap(),
        &format!("launch {} ", e.l.exe().display())
    );
}

#[test]
fn test_installed_exe_has_no_mark_of_the_web() {
    let (e, sys) = (Env::new(), FakeSystem::default());
    let exe = e.setup_exe();
    fs::write(
        format!("{}:Zone.Identifier", exe.display()),
        "[ZoneTransfer]\r\nZoneId=3\r\n",
    )
    .unwrap();
    install(
        &e.l,
        &payload(&exe),
        Options {
            autostart: true,
            desktop: false,
        },
        true,
        &sys,
        &mut |_| {},
    )
    .unwrap();
    assert!(fs::metadata(format!("{}:Zone.Identifier", exe.display())).is_ok());
    assert!(fs::metadata(format!("{}:Zone.Identifier", e.l.exe().display())).is_err());
}

#[test]
fn test_replaces_an_exe_that_cannot_be_overwritten() {
    let (e, sys) = (Env::new(), FakeSystem::default());
    fs::create_dir_all(e.l.install_dir()).unwrap();
    fs::write(e.l.exe(), b"MZ old").unwrap();
    // like a running `Anotalo.exe mcp`: readable and renamable, never writable
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(e.l.exe())
        .unwrap();
    install(
        &e.l,
        &payload(&e.setup_exe()),
        Options {
            autostart: true,
            desktop: false,
        },
        true,
        &sys,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(e.l.exe()).unwrap(), b"MZ fake setup");
    assert!(
        old_files(&e.l).len() == 1,
        "el exe en uso queda como Anotalo.exe.old-*"
    );
}

#[test]
fn test_leftovers_go_when_no_longer_in_use() {
    let (e, sys) = (Env::new(), FakeSystem::default());
    e.installed();
    let dir = e.l.install_dir();
    for f in [
        "Anotalo.exe.old-1",
        "Anotalo.exe.new",
        "vcomp140.dll.old-2",
        "notas.txt",
    ] {
        fs::write(dir.join(f), b"x").unwrap();
    }
    let busy = dir.join("Anotalo.exe.old-3");
    fs::write(&busy, b"x").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ) // en uso: no se puede borrar
        .open(&busy)
        .unwrap();
    clean_leftovers(&e.l);
    let mut left: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "Anotalo.exe",
            "Anotalo.exe.old-3",
            "notas.txt",
            "vcomp140.dll"
        ]
    );
    drop(lock);
    // la siguiente instalación borra los restos de la anterior
    install(
        &e.l,
        &payload(&e.setup_exe()),
        Options {
            autostart: true,
            desktop: false,
        },
        true,
        &sys,
        &mut |_| {},
    )
    .unwrap();
    assert!(!busy.exists());
    assert_eq!(old_files(&e.l).len(), 1, "solo el que apartó esta vez");
}

#[test]
fn test_update_closes_the_app_gracefully_then_forcibly() {
    let e = Env::new();
    let polite = FakeSystem {
        app_running: true,
        app_quits: true,
        ..Default::default()
    };
    install(
        &e.l,
        &payload(&e.setup_exe()),
        Options {
            autostart: true,
            desktop: false,
        },
        true,
        &polite,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(polite.calls()[0], "quit");
    assert!(
        !polite
            .calls()
            .iter()
            .any(|c| c.starts_with("terminate") && c != "terminate []")
    );
    let stuck = FakeSystem {
        app_running: true,
        app_quits: false,
        in_dir: vec![41, 42],
        ..Default::default()
    };
    install(
        &e.l,
        &payload(&e.setup_exe()),
        Options {
            autostart: true,
            desktop: false,
        },
        true,
        &stuck,
        &mut |_| {},
    )
    .unwrap();
    assert!(stuck.calls().contains(&"terminate [41, 42]".to_string()));
}

#[test]
fn test_transcriptor_is_closed_and_removed_after_anotalo_is_in_place() {
    let e = Env::new();
    let inno_dir = e.inno();
    let legacy_cfg = fs::read(e.l.legacy_data_dir().join("config.json")).unwrap();
    let sys = FakeSystem {
        legacy_pids: vec![7, 8],
        ..Default::default()
    };
    let found = inspect(&e.l, &sys);
    assert_eq!(
        found.inno.as_ref().map(|i| i.dir.clone()),
        Some(inno_dir.clone())
    );
    assert!(found.legacy_running && !found.app_running);
    // tenía inicio con Windows y acceso en el escritorio: Anotalo también
    assert_eq!(
        found.defaults,
        Options {
            autostart: true,
            desktop: true
        }
    );
    let mut steps = vec![];
    install(
        &e.l,
        &payload(&e.setup_exe()),
        found.defaults,
        true,
        &sys,
        &mut |s| steps.push(s),
    )
    .unwrap();
    assert_eq!(
        steps,
        [
            Step::Closing,
            Step::Copying,
            Step::Shortcuts,
            Step::Removing,
            Step::Launching,
            Step::Done
        ]
    );
    assert!(sys.calls().contains(&"terminate [7, 8]".to_string()));
    for gone in [
        "Transcriptor.exe",
        "unins000.exe",
        "unins000.dat",
        "_internal",
    ] {
        assert!(!inno_dir.join(gone).exists(), "{gone}");
    }
    assert!(inno_dir.join("mis notas.txt").exists());
    assert!(!registry::exists(&e.l.inno_key()));
    for lnk in [
        e.l.programs.join("Transcriptor.lnk"),
        e.l.desktop.join("Transcriptor.lnk"),
        e.l.startup.join("Transcriptor de llamadas.lnk"),
    ] {
        assert!(!lnk.exists(), "{}", lnk.display());
    }
    assert!(!e.l.legacy_local_dir().exists());
    assert_eq!(
        fs::read(e.l.legacy_data_dir().join("config.json")).unwrap(),
        legacy_cfg
    );
    assert!(!sys.calls().iter().any(|c| c.contains("unins000")));
    // Anotalo ocupa su lugar: menú Inicio, inicio con Windows como lo tenía Transcriptor
    assert!(e.l.start_menu_lnk().exists() && e.l.startup_lnk().exists());
    assert!(e.l.desktop_lnk().exists());
    assert_eq!(
        sys.calls().last().unwrap(),
        &format!("launch {} --minimized", e.l.exe().display())
    );
}

#[test]
fn test_silent_install_failure_relaunches_previous_app() {
    let e = Env::new();
    let inno_dir = e.inno();
    fs::create_dir_all(e.l.local_appdata.join("Programs")).unwrap();
    fs::write(e.l.install_dir(), b"un archivo donde va la carpeta").unwrap(); // copying fails
    let sys = FakeSystem {
        legacy_pids: vec![7],
        ..Default::default()
    };
    assert!(
        install(
            &e.l,
            &payload(&e.setup_exe()),
            inspect(&e.l, &sys).defaults,
            true,
            &sys,
            &mut |_| {}
        )
        .is_err()
    );
    assert!(inno_dir.join("Transcriptor.exe").exists() && registry::exists(&e.l.inno_key()));
    assert!(e.l.startup.join("Transcriptor de llamadas.lnk").exists());
    assert!(!registry::exists(&e.l.uninstall_key()));
    assert_eq!(
        sys.calls().last().unwrap(),
        &format!(
            "launch {} --minimized",
            inno_dir.join("Transcriptor.exe").display()
        )
    );
}

#[test]
fn test_failed_update_keeps_and_relaunches_the_old_anotalo() {
    let (e, sys) = (
        Env::new(),
        FakeSystem {
            app_running: true,
            app_quits: true,
            ..Default::default()
        },
    );
    fs::create_dir_all(e.l.install_dir()).unwrap();
    fs::write(e.l.exe(), b"MZ old").unwrap();
    let bad = e.l.local_appdata.join("no-existe.exe"); // reading the payload fails
    assert!(
        install(
            &e.l,
            &payload(&bad),
            Options {
                autostart: true,
                desktop: false
            },
            true,
            &sys,
            &mut |_| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(e.l.exe()).unwrap(), b"MZ old");
    assert_eq!(
        sys.calls().last().unwrap(),
        &format!("launch {} --minimized", e.l.exe().display())
    );
}

#[test]
fn test_failed_shortcuts_put_the_old_exe_back() {
    let (e, sys) = (
        Env::new(),
        FakeSystem {
            app_running: true,
            app_quits: true,
            ..Default::default()
        },
    );
    let inno_dir = e.inno();
    fs::create_dir_all(e.l.install_dir()).unwrap();
    fs::write(e.l.exe(), b"MZ old").unwrap();
    fs::write(e.l.install_dir().join("vcomp140.dll"), b"MZ old vcomp").unwrap();
    fs::create_dir_all(e.l.start_menu_lnk()).unwrap(); // the Start-menu shortcut cannot be written
    let mut steps = vec![];
    assert!(
        install(
            &e.l,
            &payload(&e.setup_exe()),
            Options {
                autostart: true,
                desktop: false
            },
            true,
            &sys,
            &mut |s| steps.push(s)
        )
        .is_err()
    );
    assert_eq!(steps, [Step::Closing, Step::Copying, Step::Shortcuts]);
    assert_eq!(fs::read(e.l.exe()).unwrap(), b"MZ old");
    assert_eq!(
        fs::read(e.l.install_dir().join("vcomp140.dll")).unwrap(),
        b"MZ old vcomp"
    );
    assert!(old_files(&e.l).is_empty());
    assert!(inno_dir.join("Transcriptor.exe").exists() && registry::exists(&e.l.inno_key()));
    assert_eq!(
        sys.calls().last().unwrap(),
        &format!("launch {} --minimized", e.l.exe().display())
    );
}

#[test]
fn test_autostart_follows_transcriptor_choice() {
    let sys = FakeSystem::default();
    let e = Env::new();
    e.inno(); // with "Transcriptor de llamadas.lnk"
    assert!(inspect(&e.l, &sys).defaults.autostart);
    fs::remove_file(e.l.startup.join("Transcriptor de llamadas.lnk")).unwrap();
    fs::write(
        e.l.legacy_data_dir().join("config.json"),
        r#"{"autostart": false}"#,
    )
    .unwrap();
    assert!(!inspect(&e.l, &sys).defaults.autostart);
    let fresh = Env::new();
    assert!(inspect(&fresh.l, &sys).defaults.autostart);
    install(
        &fresh.l,
        &payload(&fresh.setup_exe()),
        Options {
            autostart: false,
            desktop: true,
        },
        true,
        &sys,
        &mut |_| {},
    )
    .unwrap();
    let again = inspect(&fresh.l, &sys);
    assert_eq!(
        (again.defaults, again.anotalo.as_deref()),
        (
            Options {
                autostart: false,
                desktop: true
            },
            Some("9.9.9")
        )
    );
}

#[test]
fn test_python_folders_holding_recordings_are_kept() {
    let (e, sys) = (Env::new(), FakeSystem::default());
    let inno_dir = e.inno();
    // grabaciones (por raro que sea) dentro de la caché de CUDA y de `_internal`
    let in_cuda = e.l.legacy_local_dir().join("cuda").join("Llamadas");
    let in_internal = inno_dir.join("_internal").join("Llamadas");
    for (cfg, rec) in [
        (e.l.legacy_data_dir().join("config.json"), &in_cuda),
        (e.l.data_dir().join("config.json"), &in_internal),
    ] {
        fs::create_dir_all(rec).unwrap();
        fs::write(rec.join("a.md"), "# a").unwrap();
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        fs::write(&cfg, serde_json::json!({"output_dir": rec}).to_string()).unwrap();
    }
    install(
        &e.l,
        &payload(&e.setup_exe()),
        inspect(&e.l, &sys).defaults,
        true,
        &sys,
        &mut |_| {},
    )
    .unwrap();
    assert!(in_cuda.join("a.md").exists() && in_internal.join("a.md").exists());
    assert!(!inno_dir.join("Transcriptor.exe").exists() && !registry::exists(&e.l.inno_key()));
}

#[test]
fn test_install_never_touches_user_data() {
    let (e, sys) = (Env::new(), FakeSystem::default());
    let files = e.user_data(); // %APPDATA%\Anotalo\{config.json,voces.json}, <root>\Documents\Llamadas\a.md, .papelera\b.md
    let before = hashes(&files);
    install(
        &e.l,
        &payload(&e.setup_exe()),
        Options {
            autostart: true,
            desktop: false,
        },
        true,
        &sys,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(hashes(&files), before);
}

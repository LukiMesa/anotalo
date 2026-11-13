//! Inicio con Windows: el acceso directo `Anotalo.lnk` de la carpeta Inicio. Port de
//! `tests/test_autostart.py` sobre `Layout::for_test` ("frozen" de Python = el perfil instalado).
mod common;

use anotalo_core::config::Settings;
use anotalo_core::paths::Profile;
use anotalo_setup::autostart::{disable, enable, ensure_autostart, is_enabled};
use anotalo_winsys::shortcut;
use common::Env;

#[test]
fn test_autostart_enable_disable() {
    let e = Env::new();
    assert!(!is_enabled(&e.l));
    enable(&e.l).unwrap();
    assert!(e.l.startup_lnk().exists());
    assert!(is_enabled(&e.l));
    disable(&e.l).unwrap();
    assert!(!is_enabled(&e.l));
    disable(&e.l).unwrap(); // sin acceso directo tampoco es un error
}

#[test]
fn test_enable_points_at_the_installed_exe() {
    let e = Env::new();
    enable(&e.l).unwrap();
    let s = shortcut::read(&e.l.startup_lnk()).unwrap();
    assert_eq!(
        (
            s.target,
            s.args.as_str(),
            s.workdir,
            s.description.as_str(),
            s.aumid
        ),
        (e.l.exe(), "--minimized", e.l.install_dir(), "Anotalo", None)
    );
}

#[test]
fn test_dev_never_touches_shortcut() {
    let e = Env::new();
    let mut s = Settings::defaults(Profile::Dev);
    assert!(!ensure_autostart(&mut s, Profile::Dev, &e.l));
    assert!(!is_enabled(&e.l));
    assert!(s.autostart && !s.autostart_configured);
    // (test_target_for_current_run_dev_is_none) con un acceso directo puesto, tampoco lo toca
    enable(&e.l).unwrap();
    let mut s = Settings::defaults(Profile::Dev);
    s.autostart = false;
    assert!(!ensure_autostart(&mut s, Profile::Dev, &e.l));
    assert!(is_enabled(&e.l));
    assert!(!s.autostart && !s.autostart_configured);
}

#[test]
fn test_frozen_first_run_reads_existing_shortcut() {
    let e = Env::new();
    enable(&e.l).unwrap(); // lo creó el instalador
    let mut s = Settings::defaults(Profile::Installed);
    s.autostart = false;
    assert!(ensure_autostart(&mut s, Profile::Installed, &e.l));
    assert!(s.autostart && s.autostart_configured);
}

#[test]
fn test_frozen_first_run_without_shortcut_sets_false() {
    let e = Env::new();
    let mut s = Settings::defaults(Profile::Installed);
    assert!(ensure_autostart(&mut s, Profile::Installed, &e.l));
    assert!(!s.autostart && s.autostart_configured);
    assert!(!is_enabled(&e.l));
}

#[test]
fn test_frozen_syncs_with_shortcut_every_start() {
    let e = Env::new();
    let mut s = Settings::defaults(Profile::Installed);
    s.autostart = true;
    s.autostart_configured = true;
    // el instalador (actualización) quitó el acceso directo: el ajuste lo refleja
    assert!(ensure_autostart(&mut s, Profile::Installed, &e.l));
    assert!(!s.autostart);
    assert!(!ensure_autostart(&mut s, Profile::Installed, &e.l));
}

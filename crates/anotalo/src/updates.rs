//! Actualizaciones e inicio con Windows en la app de la bandeja: solo la versión instalada; la de
//! desarrollo nunca busca actualizaciones ni toca el inicio con Windows (spec §2.1).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anotalo_core::app::{App, AppEvent, Applier, EventSink};
use anotalo_core::config::Settings;
use anotalo_core::models::HttpFetch;
use anotalo_core::notify::Notifier;
use anotalo_core::paths::Profile;
use anotalo_core::updater::{
    EVERY, FIRST_CHECK, INSTALL_TIMEOUT, INSTALLER_ARGS, UpdateDeps, UpdateManager, Version,
    announce_if_updated, clear_downloads, download, download_dir, fetch_release,
};
use anotalo_setup::autostart;
use anotalo_setup::install::clean_leftovers;
use anotalo_setup::layout::Layout;
use anotalo_winsys::process;
use serde_json::json;

const ONLY_INSTALLED: &str = "Solo en la versión instalada";

/// El actualizador, solo en la versión instalada; en desarrollo ni se llama a `deps`.
pub fn updater_for(p: Profile, deps: impl FnOnce() -> UpdateDeps) -> Option<Arc<UpdateManager>> {
    (p == Profile::Installed).then(|| UpdateManager::new(deps()))
}

/// El ajuste "Iniciar con Windows": crea o quita `Anotalo.lnk` en la carpeta Inicio. En desarrollo
/// (o sin las carpetas del usuario) no se puede.
pub fn autostart_applier(p: Profile, l: Option<Layout>) -> Applier {
    Box::new(move |v| {
        if p == Profile::Dev {
            return Err(ONLY_INSTALLED.into());
        }
        let Some(l) = &l else {
            return Err("No se encontró la carpeta de inicio de Windows".into());
        };
        if v.as_bool().unwrap_or(false) {
            autostart::enable(l)
        } else {
            autostart::disable(l)
        }
    })
}

/// Al arrancar la versión instalada: el ajuste "Iniciar con Windows" sigue al acceso directo (lo
/// pone o lo quita el instalador), se avisa una vez de la versión nueva tras actualizar y se
/// borran los restos de actualizaciones (el exe anterior apartado y los instaladores descargados,
/// también los de Python). Devuelve true si cambió `s` (hay que guardar).
pub fn on_startup(
    p: Profile,
    l: Option<&Layout>,
    s: &mut Settings,
    version: &str,
    n: &dyn Notifier,
) -> bool {
    if p == Profile::Dev {
        return false;
    }
    let mut changed = false;
    if let Some(l) = l {
        changed |= autostart::ensure_autostart(s, p, l);
        clean_leftovers(l);
        clear_downloads(&[
            l.temp.join("Anotalo-update"),
            l.temp.join("Transcriptor-update"),
        ]);
    }
    changed |= announce_if_updated(s, version, n);
    changed
}

/// El actualizador de la versión instalada, ya comprobando en segundo plano (al minuto de
/// arrancar y cada 6 h, si "Buscar actualizaciones automáticamente" está activado). None en
/// desarrollo.
pub fn start_updater(
    p: Profile,
    app: &Arc<App>,
    events: EventSink,
    notifier: Arc<dyn Notifier>,
    settings: &Settings,
) -> Option<Arc<UpdateManager>> {
    let version = env!("CARGO_PKG_VERSION");
    // una versión de prueba también ve las de prueba: así se pasa de una a la siguiente
    let prereleases = Version::parse(version).is_some_and(|v| v.is_prerelease());
    let updater = updater_for(p, || {
        let http = Arc::new(HttpFetch::default());
        let http2 = http.clone();
        let (a1, a2, a3) = (app.clone(), app.clone(), app.clone());
        UpdateDeps {
            current: version.into(),
            is_recording: Box::new(move || a1.recording_active()),
            notifier,
            on_change: Box::new(move |snapshot| events(AppEvent::Update { snapshot })),
            set_updating: Box::new(move |on| a2.set_updating(on)),
            fetch: Box::new(move || fetch_release(&*http, prereleases)),
            download: Box::new(move |rel, progress| {
                download(rel, &download_dir(), &*http2, progress)
            }),
            launch: Box::new(|path| {
                let dir = path.parent().unwrap_or(Path::new("."));
                process::spawn_detached(path, &INSTALLER_ARGS, dir).map(|_| ())
            }),
            notified_version: settings.update_notified.clone(),
            on_notified: Box::new(move |v| {
                if let Err(e) = a3.apply_setting("update_notified", json!(v)) {
                    log::warn!("No se pudo anotar el aviso de la versión {v}: {e}");
                }
            }),
            install_timeout: INSTALL_TIMEOUT,
        }
    })?;
    let enabled = Arc::new(AtomicBool::new(settings.auto_update_check));
    let e = enabled.clone();
    app.set_after_apply(
        "auto_update_check",
        Box::new(move |v| e.store(v.as_bool().unwrap_or(true), Ordering::SeqCst)),
    );
    updater.start_background(FIRST_CHECK, EVERY, enabled);
    log::info!(
        "Actualizaciones: al arrancar y cada 6 h ({}{})",
        if prereleases {
            "también las de prueba"
        } else {
            "solo las estables"
        },
        if settings.auto_update_check {
            ""
        } else {
            "; ahora desactivadas"
        }
    );
    Some(updater)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anotalo_core::config::Settings;
    use anotalo_core::notify::Notifier;
    use anotalo_core::paths::Profile;
    use anotalo_setup::layout::Layout;
    use serde_json::Value;
    use std::path::Path;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Notes(Mutex<Vec<(String, String)>>);

    impl Notifier for Notes {
        fn notify(&self, title: &str, body: &str, _open: Option<&Path>) {
            self.0.lock().unwrap().push((title.into(), body.into()));
        }
    }

    #[test]
    fn test_dev_profile_has_no_updater_or_autostart() {
        assert!(updater_for(Profile::Dev, || unreachable!()).is_none());
        let f = autostart_applier(Profile::Dev, None);
        assert_eq!(
            f(&Value::Bool(true)),
            Err("Solo en la versión instalada".to_string())
        );
    }

    #[test]
    fn test_installed_autostart_applier_sets_the_shortcut() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::for_test(d.path(), r"Software\AnotaloTest\no-se-usa");
        let f = autostart_applier(Profile::Installed, Some(l.clone()));
        f(&Value::Bool(true)).unwrap();
        assert!(l.startup_lnk().exists());
        f(&Value::Bool(false)).unwrap();
        assert!(!l.startup_lnk().exists());
        // sin las carpetas del usuario no se puede
        assert!(autostart_applier(Profile::Installed, None)(&Value::Bool(true)).is_err());
    }

    #[test]
    fn test_startup_syncs_autostart_announces_and_cleans() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::for_test(d.path(), r"Software\AnotaloTest\no-se-usa");
        let update = l.temp.join("Anotalo-update");
        let python = l.temp.join("Transcriptor-update");
        for f in [
            update.join("Anotalo-Setup-1.0.0.exe"),
            python.join("Anotalo-Setup-1.0.0.exe"),
            l.install_dir().join("Anotalo.exe.old-1"),
            l.install_dir().join("Anotalo.exe"),
        ] {
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, b"MZ").unwrap();
        }
        let notes = Notes::default();

        // desarrollo: no toca nada
        let mut s = Settings::defaults(Profile::Dev);
        assert!(!on_startup(Profile::Dev, Some(&l), &mut s, "9.9.9", &notes));
        assert!(update.exists() && !s.autostart_configured && s.last_run_version.is_empty());

        // instalada, tras actualizar de la 9.9.8: el ajuste sigue al acceso directo (no hay),
        // se avisa una vez y se borran los restos
        let mut s = Settings::defaults(Profile::Installed);
        s.last_run_version = "9.9.8".into();
        assert!(on_startup(
            Profile::Installed,
            Some(&l),
            &mut s,
            "9.9.9",
            &notes
        ));
        assert!(!s.autostart && s.autostart_configured);
        assert_eq!(s.last_run_version, "9.9.9");
        assert_eq!(
            *notes.0.lock().unwrap(),
            [(
                "Anotalo actualizado".to_string(),
                "Ya tienes la versión 9.9.9".to_string()
            )]
        );
        assert!(!update.exists() && !python.exists());
        assert!(!l.install_dir().join("Anotalo.exe.old-1").exists());
        assert!(l.install_dir().join("Anotalo.exe").exists());
        // la siguiente vez no hay nada que guardar
        assert!(!on_startup(
            Profile::Installed,
            Some(&l),
            &mut s,
            "9.9.9",
            &notes
        ));
    }
}

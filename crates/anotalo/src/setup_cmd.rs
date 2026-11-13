//! El exe como instalador y desinstalador (spec §5.8; `anotalo_setup::cmdline` decide cuándo).
//!
//! Siempre sobre la instalación del usuario (`Layout::current`), se lance desde donde se lance.
//! Registra en `%APPDATA%\Anotalo\setup.log`. En silencio no pregunta nada; si no, abre su
//! ventana (`setup_ui`), o cuadros de mensaje sin WebView2.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anotalo_core::applog;
use anotalo_core::config::read_output_dir;
use anotalo_core::paths::Profile;
use anotalo_setup::cmdline::SetupMode;
use anotalo_setup::install::{Found, Options, Payload, inspect, install};
use anotalo_setup::layout::Layout;
use anotalo_setup::legacy::InnoInstall;
use anotalo_setup::system::RealSystem;
use anotalo_setup::uninstall::{schedule_self_delete, uninstall};
use anotalo_winsys::{dialog, instance, webview2, webview2_loader};

use crate::setup_ui::{self, Outcome, Runner};

/// Los títulos de la ventana (barra de tareas, Alt+Tab).
const INSTALL_TITLE: &str = "Instalar Anotalo";
const UNINSTALL_TITLE: &str = "Desinstalar Anotalo";

/// La `vcomp140.dll` de Microsoft (firmada por Microsoft), que el instalador pone junto al exe.
pub static VCOMP140: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vcomp140.dll"));

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Un solo instalador o desinstalador a la vez.
const SETUP_MUTEX: &str = "AnotaloSetupMutex";
const LOG_MAX_BYTES: u64 = 1_000_000;
const LOG_BACKUPS: usize = 1;

pub const WEBVIEW2_MISSING: &str = "Falta Microsoft Edge WebView2 Runtime, necesario para la ventana de Anotalo.\n\n¿Abrir la página de descarga de Microsoft? Anotalo se instala igualmente.";
pub const DONE: &str =
    "Anotalo está instalado. Lo tienes en la bandeja del sistema, junto al reloj.";
const ALREADY_INSTALLING: &str = "Ya se está instalando Anotalo.";
const ALREADY_SETUP: &str = "Ya hay otro instalador de Anotalo en marcha.";

/// La pregunta antes de instalar. Solo promete el inicio con Windows si va a quedar activado.
pub fn ask_install_text(version: &str, replaces_transcriptor: bool, autostart: bool) -> String {
    let transcriptor = if replaces_transcriptor {
        "Sustituye a Transcriptor: tus grabaciones y ajustes se conservan.\n\n"
    } else {
        ""
    };
    let with = if autostart {
        "con acceso en el menú Inicio e inicio con Windows"
    } else {
        "con acceso en el menú Inicio"
    };
    format!(
        "Se instalará Anotalo {version} en tu usuario, sin permisos de administrador, {with}.\n\n{transcriptor}¿Instalar ahora?"
    )
}

pub fn ask_uninstall_text(recordings: &Path) -> String {
    format!(
        "¿Desinstalar Anotalo?\n\nTus grabaciones de {} no se borran, ni tus ajustes.",
        recordings.display()
    )
}

pub fn uninstalled_text(recordings: &Path) -> String {
    format!(
        "Anotalo se ha desinstalado. Tus grabaciones siguen en {}.",
        recordings.display()
    )
}

/// Sin las instrucciones que necesita el motor de transcripción, Anotalo no arrancaría.
pub fn unsupported_cpu_text(missing: &[&str]) -> String {
    let names: Vec<String> = missing.iter().map(|m| m.to_uppercase()).collect();
    format!(
        "Anotalo no puede funcionar en este PC: a su procesador le faltan instrucciones \
         que necesita la transcripción ({}). No he cambiado nada.",
        names.join(", ")
    )
}

pub fn run(mode: SetupMode) -> ExitCode {
    let silent = match mode {
        SetupMode::Install { silent } | SetupMode::Uninstall { silent } => silent,
    };
    // fuera de la carpeta de quien lo lanzó (la de Transcriptor, si lo lanza su actualizador): si
    // no, esa carpeta no se puede borrar
    let system32 = std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("System32");
    if let Err(e) = std::env::set_current_dir(&system32) {
        eprintln!("No se pudo cambiar de carpeta: {e}");
    }
    // nunca enganchado a una terminal: un Ctrl+C en ella cortaría la instalación a medias. El
    // registro va a setup.log, y a la salida que nos hayan dado (redirigida), si la hay.
    let to_console = crate::console::has_output();
    let layout = match Layout::current() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("No se encontraron las carpetas del usuario: {e}");
            if !silent {
                dialog::tell(&format!("No se pudo instalar Anotalo: {e}"));
            }
            return ExitCode::from(1);
        }
    };
    if let Err(e) = applog::init_named(
        &layout.data_dir(),
        "setup.log",
        LOG_MAX_BYTES,
        LOG_BACKUPS,
        to_console,
    ) {
        eprintln!("No se pudo abrir {}: {e}", layout.setup_log().display());
    }
    std::panic::set_hook(Box::new(|info| log::error!("Fallo interno: {info}")));
    let exe = std::env::current_exe().unwrap_or_default();
    log::info!("Anotalo {VERSION}: {mode:?} desde {}", exe.display());

    let Some(_one) = instance::acquire(SETUP_MUTEX) else {
        log::info!("Ya hay otro instalador en marcha: no se hace nada");
        if !silent {
            dialog::tell(match mode {
                SetupMode::Install { .. } => ALREADY_INSTALLING,
                SetupMode::Uninstall { .. } => ALREADY_SETUP,
            });
        }
        return ExitCode::SUCCESS;
    };
    let code = match mode {
        SetupMode::Install { silent } => run_install(&layout, &exe, silent),
        SetupMode::Uninstall { silent } => run_uninstall(&layout, &exe, silent),
    };
    log::logger().flush();
    code
}

fn run_install(l: &Layout, exe: &Path, silent: bool) -> ExitCode {
    run_install_with(l, exe, silent, &anotalo_whisper::missing_cpu_features)
}

/// `cpu_missing`: las instrucciones que le faltan al procesador (inyectable para los tests).
fn run_install_with(
    l: &Layout,
    exe: &Path,
    silent: bool,
    cpu_missing: &dyn Fn() -> Vec<&'static str>,
) -> ExitCode {
    // antes de tocar nada: si Anotalo no puede arrancar aquí, Transcriptor se queda como está
    let missing = cpu_missing();
    if !missing.is_empty() {
        let text = unsupported_cpu_text(&missing);
        log::error!("{text}");
        if !silent {
            dialog::tell(&text);
        }
        return ExitCode::from(1);
    }
    let sys = RealSystem::new();
    let found = inspect(l, &sys);
    log::info!(
        "Instalado: {:?}; Transcriptor: {:?}; app abierta: {}; Transcriptor abierto: {}; opciones: {:?}",
        found.anotalo,
        found.inno.as_ref().map(|i| i.dir.display().to_string()),
        found.app_running,
        found.legacy_running,
        found.defaults
    );
    if !silent && webview2_loader::find_runtime().is_ok() {
        match install_window(l, exe, &found) {
            Ok(Some(Err(_))) => return ExitCode::from(1),
            Ok(_) => return ExitCode::SUCCESS,
            Err(e) => log::warn!("Sin ventana de instalación ({e}): sigo con cuadros de mensaje"),
        }
    }
    if !silent {
        // el mismo criterio que la ventana de la app: si su cargador no encuentra el runtime
        if let Err(e) = webview2_loader::find_runtime() {
            log::warn!("Sin WebView2: {e}");
            if dialog::ask(WEBVIEW2_MISSING) {
                dialog::open_url(webview2::DOWNLOAD_URL);
            }
        }
        let question = ask_install_text(VERSION, found.inno.is_some(), found.defaults.autostart);
        if !dialog::ask(&question) {
            log::info!("Instalación cancelada");
            return ExitCode::SUCCESS;
        }
    }
    let payload = Payload {
        exe,
        vcomp: VCOMP140,
        version: VERSION,
    };
    let result = install(l, &payload, found.defaults, silent, &sys, &mut |s| {
        log::info!("Paso: {s:?}")
    });
    match result {
        Ok(()) => {
            if !silent {
                dialog::tell(DONE);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            log::error!("No se pudo instalar Anotalo: {e}");
            if !silent {
                dialog::tell(&format!("No se pudo instalar Anotalo: {e}"));
            }
            ExitCode::from(1)
        }
    }
}

/// La ventana del instalador. Err si no se pudo abrir.
fn install_window(l: &Layout, exe: &Path, found: &Found) -> Result<Outcome, String> {
    let recordings = read_output_dir(&l.data_dir().join("config.json"), Profile::Installed);
    let info = setup_ui::info("install", found, VERSION, &recordings);
    let (l, exe, defaults) = (l.clone(), exe.to_path_buf(), found.defaults);
    let runner: Runner = Box::new(move |opts, step| {
        let options = Options {
            autostart: opts["autostart"].as_bool().unwrap_or(defaults.autostart),
            desktop: opts["desktop"].as_bool().unwrap_or(defaults.desktop),
        };
        let payload = Payload {
            exe: &exe,
            vcomp: VCOMP140,
            version: VERSION,
        };
        // no en silencio: al terminar, la app se abre con su ventana
        install(&l, &payload, options, false, &RealSystem::new(), &mut |s| {
            log::info!("Paso: {s:?}");
            step(setup_ui::step_name(s));
        })
        .map(|()| None)
    });
    setup_ui::run_window(INSTALL_TITLE, info, runner, true)
}

/// La ventana del desinstalador. Err si no se pudo abrir.
fn uninstall_window(l: &Layout, exe: &Path, recordings: &Path) -> Result<Outcome, String> {
    let found = Found {
        anotalo: None,
        inno: None,
        app_running: false,
        legacy_running: false,
        defaults: Options {
            autostart: false,
            desktop: false,
        },
    };
    let info = setup_ui::info("uninstall", &found, VERSION, recordings);
    let (l, exe) = (l.clone(), exe.to_path_buf());
    let runner: Runner = Box::new(move |opts, step| {
        step("removing");
        let purge = opts["purge"].as_bool().unwrap_or(false);
        let removal = uninstall(&l, purge, &RealSystem::new(), &exe);
        for p in &removal.problems {
            log::warn!("Al desinstalar: {p}");
        }
        Ok(removal.deferred)
    });
    setup_ui::run_window(UNINSTALL_TITLE, info, runner, false)
}

/// Borra este exe al salir, si el desinstalador lo dejó para el final.
fn delete_on_exit(deferred: Option<&Path>) {
    if let Some(own) = deferred {
        let dir = own.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Err(e) = schedule_self_delete(own, &dir) {
            log::error!("{e}");
        }
    }
}

fn run_uninstall(l: &Layout, exe: &Path, silent: bool) -> ExitCode {
    let recordings = read_output_dir(&l.data_dir().join("config.json"), Profile::Installed);
    if !silent && webview2_loader::find_runtime().is_ok() {
        match uninstall_window(l, exe, &recordings) {
            Ok(outcome) => {
                // lo último antes de salir, con la ventana ya cerrada
                if let Some(Ok(deferred)) = &outcome {
                    delete_on_exit(deferred.as_deref());
                }
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                log::warn!("Sin ventana de desinstalación ({e}): sigo con cuadros de mensaje")
            }
        }
    }
    if !silent && !dialog::ask(&ask_uninstall_text(&recordings)) {
        log::info!("Desinstalación cancelada");
        return ExitCode::SUCCESS;
    }
    let removal = uninstall(l, false, &RealSystem::new(), exe);
    for p in &removal.problems {
        log::warn!("Al desinstalar: {p}");
    }
    if !silent {
        dialog::tell(&uninstalled_text(&recordings));
    }
    // lo último antes de salir: el cmd espera ~2 s y luego borra este exe y su carpeta
    delete_on_exit(removal.deferred.as_deref());
    ExitCode::SUCCESS
}

/// `anotalo --setup-preview install|uninstall` (solo la versión de desarrollo): la ventana del
/// instalador con datos de ejemplo y pasos simulados (400 ms cada uno), para ver cómo queda. No
/// instala ni toca nada; con `ANOTALO_PREVIEW_FAIL=1`, la instalación falla.
pub fn preview(mode: &str) -> ExitCode {
    let found = Found {
        anotalo: std::env::var("ANOTALO_PREVIEW_INSTALLED").ok(),
        inno: Some(InnoInstall {
            dir: std::env::temp_dir().join("Transcriptor"),
        }),
        app_running: true,
        legacy_running: false,
        defaults: Options {
            autostart: true,
            desktop: false,
        },
    };
    let recordings = anotalo_core::paths::default_output_dir(Profile::Installed);
    let info = setup_ui::info(mode, &found, VERSION, &recordings);
    let uninstalling = mode == "uninstall";
    let fail = std::env::var("ANOTALO_PREVIEW_FAIL").is_ok_and(|v| v == "1");
    let runner: Runner = Box::new(move |opts, step| {
        log::info!("Vista previa: {opts}");
        let steps: &[&'static str] = if uninstalling {
            &["removing", "removing", "removing"]
        } else {
            &["closing", "copying", "shortcuts", "removing", "launching"]
        };
        for (i, s) in steps.iter().enumerate() {
            step(s);
            std::thread::sleep(Duration::from_millis(400));
            if fail && i == 1 {
                return Err("no queda espacio en el disco (simulado)".into());
            }
        }
        Ok(None)
    });
    let title = if uninstalling {
        UNINSTALL_TITLE
    } else {
        INSTALL_TITLE
    };
    match setup_ui::run_window(title, info, runner, !uninstalling) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// En un procesador sin las instrucciones del motor (Celeron N, Sandy Bridge…), Anotalo no
    /// arrancaría: el instalador no toca nada, así Transcriptor sigue funcionando.
    #[test]
    fn test_unsupported_cpu_touches_nothing() {
        let d = tempfile::tempdir().unwrap();
        let key = format!(r"Software\AnotaloTest\cpu-{}", std::process::id());
        let l = Layout::for_test(d.path(), &key);
        let code = run_install_with(&l, Path::new(r"C:\no\existe.exe"), true, &|| vec!["avx2"]);
        assert_eq!(code, ExitCode::from(1));
        assert_eq!(
            std::fs::read_dir(d.path()).unwrap().count(),
            0,
            "nada creado"
        );
        assert_eq!(
            unsupported_cpu_text(&["avx2", "fma"]),
            "Anotalo no puede funcionar en este PC: a su procesador le faltan instrucciones \
             que necesita la transcripción (AVX2, FMA). No he cambiado nada."
        );
    }

    #[test]
    fn embedded_vcomp_is_a_dll() {
        assert!(VCOMP140.len() > 100_000, "{} bytes", VCOMP140.len());
        assert_eq!(&VCOMP140[..2], b"MZ");
    }

    #[test]
    fn install_question() {
        assert_eq!(
            ask_install_text("1.0.0", false, true),
            "Se instalará Anotalo 1.0.0 en tu usuario, sin permisos de administrador, con acceso en el menú Inicio e inicio con Windows.\n\n¿Instalar ahora?"
        );
        assert_eq!(
            ask_install_text("1.0.0-alpha.2", true, true),
            "Se instalará Anotalo 1.0.0-alpha.2 en tu usuario, sin permisos de administrador, con acceso en el menú Inicio e inicio con Windows.\n\nSustituye a Transcriptor: tus grabaciones y ajustes se conservan.\n\n¿Instalar ahora?"
        );
        assert_eq!(
            ask_install_text("1.0.0", false, false),
            "Se instalará Anotalo 1.0.0 en tu usuario, sin permisos de administrador, con acceso en el menú Inicio.\n\n¿Instalar ahora?"
        );
    }

    #[test]
    fn uninstall_texts() {
        let dir = Path::new(r"C:\Users\a\Documents\Llamadas");
        assert_eq!(
            ask_uninstall_text(dir),
            "¿Desinstalar Anotalo?\n\nTus grabaciones de C:\\Users\\a\\Documents\\Llamadas no se borran, ni tus ajustes."
        );
        assert_eq!(
            uninstalled_text(dir),
            "Anotalo se ha desinstalado. Tus grabaciones siguen en C:\\Users\\a\\Documents\\Llamadas."
        );
        assert_eq!(
            WEBVIEW2_MISSING,
            "Falta Microsoft Edge WebView2 Runtime, necesario para la ventana de Anotalo.\n\n¿Abrir la página de descarga de Microsoft? Anotalo se instala igualmente."
        );
        assert_eq!(
            DONE,
            "Anotalo está instalado. Lo tienes en la bandeja del sistema, junto al reloj."
        );
    }
}

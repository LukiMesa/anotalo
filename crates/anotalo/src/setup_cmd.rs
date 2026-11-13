//! El exe como instalador y desinstalador (spec §5.8; `anotalo_setup::cmdline` decide cuándo).
//!
//! Siempre sobre la instalación del usuario (`Layout::current`), se lance desde donde se lance.
//! Registra en `%APPDATA%\Anotalo\setup.log`. En silencio no pregunta nada; si no, hasta que haya
//! ventana de instalación (M4 Task 14), usa cuadros de mensaje.

use std::path::Path;
use std::process::ExitCode;

use anotalo_core::applog;
use anotalo_core::config::read_output_dir;
use anotalo_core::paths::Profile;
use anotalo_setup::cmdline::SetupMode;
use anotalo_setup::install::{Payload, inspect, install};
use anotalo_setup::layout::Layout;
use anotalo_setup::system::RealSystem;
use anotalo_setup::uninstall::{schedule_self_delete, uninstall};
use anotalo_winsys::{dialog, instance, webview2, webview2_loader};

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

pub fn run(mode: SetupMode) -> ExitCode {
    let silent = match mode {
        SetupMode::Install { silent } | SetupMode::Uninstall { silent } => silent,
    };
    // lanzado desde una terminal, el registro también sale por ella
    let to_console = crate::console::attach_parent();
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

fn run_uninstall(l: &Layout, exe: &Path, silent: bool) -> ExitCode {
    let recordings = read_output_dir(&l.data_dir().join("config.json"), Profile::Installed);
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
    if let Some(own) = &removal.deferred {
        let dir = own.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Err(e) = schedule_self_delete(own, &dir) {
            log::error!("{e}");
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

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

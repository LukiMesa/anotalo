//! Carpetas de Anotalo. La versión instalada y la de desarrollo usan carpetas distintas para no
//! pisarse en el mismo PC (como la app de Python con su "-dev").

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Installed,
    Dev,
}

/// `ANOTALO_PROFILE=installed|dev` manda; si no, es la instalada si el exe está en
/// `%LOCALAPPDATA%\Programs\Anotalo`.
pub fn profile() -> Profile {
    let env = std::env::var("ANOTALO_PROFILE").ok();
    let exe = std::env::current_exe().unwrap_or_default();
    profile_from(env.as_deref(), &exe, &local_appdata())
}

pub fn profile_from(env: Option<&str>, exe: &Path, localappdata: &Path) -> Profile {
    match env.map(|v| v.trim().to_lowercase()).as_deref() {
        Some("dev") => return Profile::Dev,
        Some("installed") => return Profile::Installed,
        _ => {}
    }
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let install = norm(&install_dir_in(localappdata));
    match exe.parent() {
        Some(dir) if norm(dir) == install => Profile::Installed,
        _ => Profile::Dev,
    }
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default"))
}

fn appdata() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Roaming"))
}

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Local"))
}

/// `%LOCALAPPDATA%\Programs\Anotalo`: donde instala el instalador (por usuario).
pub fn install_dir() -> PathBuf {
    install_dir_in(&local_appdata())
}

pub fn install_dir_in(localappdata: &Path) -> PathBuf {
    localappdata.join("Programs").join("Anotalo")
}

/// `%APPDATA%\Anotalo` (ajustes, log, voces), o `Anotalo-dev`.
pub fn data_dir(p: Profile) -> PathBuf {
    data_dir_in(&appdata(), p)
}

pub fn data_dir_in(appdata: &Path, p: Profile) -> PathBuf {
    appdata.join(match p {
        Profile::Installed => "Anotalo",
        Profile::Dev => "Anotalo-dev",
    })
}

/// La carpeta de la app de Python (`CallTranscriber`), de la que se copian los ajustes.
pub fn legacy_data_dir(p: Profile) -> PathBuf {
    legacy_data_dir_in(&appdata(), p)
}

pub fn legacy_data_dir_in(appdata: &Path, p: Profile) -> PathBuf {
    appdata.join(match p {
        Profile::Installed => "CallTranscriber",
        Profile::Dev => "CallTranscriber-dev",
    })
}

/// `Documentos\Llamadas`, o `Llamadas (dev)` (la misma ruta que la app de Python).
pub fn default_output_dir(p: Profile) -> PathBuf {
    default_output_dir_in(&home(), p)
}

pub fn default_output_dir_in(home: &Path, p: Profile) -> PathBuf {
    home.join("Documents").join(match p {
        Profile::Installed => "Llamadas",
        Profile::Dev => "Llamadas (dev)",
    })
}

/// (mutex de instancia única, evento para pedir que se muestre la ventana).
pub fn instance_names(p: Profile) -> (&'static str, &'static str) {
    match p {
        Profile::Installed => ("AnotaloMutex", "AnotaloShow"),
        Profile::Dev => ("AnotaloDevMutex", "AnotaloDevShow"),
    }
}

/// El evento con nombre con el que el instalador pide a la app que se cierre (guardando la
/// grabación en curso).
pub fn quit_event(p: Profile) -> &'static str {
    match p {
        Profile::Installed => "AnotaloQuit",
        Profile::Dev => "AnotaloDevQuit",
    }
}

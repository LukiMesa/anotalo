//! El Transcriptor instalado con Inno (Python 0.6.x): encontrarlo y quitarlo sin ejecutar su
//! desinstalador (`unins000.exe`, sin firmar). Solo se borra lo que puso Inno: el usuario pudo
//! elegir una carpeta con más cosas. Sus ajustes (`%APPDATA%\CallTranscriber`) y las grabaciones
//! se quedan.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anotalo_core::config::read_output_dir;
use anotalo_core::paths::Profile;
use anotalo_winsys::registry;
use serde_json::Value;

use crate::layout::{LEGACY_EXE, LEGACY_LNK, LEGACY_STARTUP_LNK, Layout};

/// Dónde está instalado Transcriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InnoInstall {
    pub dir: PathBuf,
}

/// La instalación de Inno: la carpeta de su clave de desinstalación (`InstallLocation`) o, sin
/// clave, la carpeta por defecto si tiene `_internal\`.
pub fn find_inno(l: &Layout) -> Option<InnoInstall> {
    if registry::exists(&l.inno_key()) {
        let location = registry::get_string(&l.inno_key(), "InstallLocation").unwrap_or_default();
        let dir = PathBuf::from(location.trim().trim_end_matches(['\\', '/']));
        if dir.is_absolute() && dir.parent().is_some() {
            return Some(InnoInstall { dir });
        }
        // la clave está, pero sin una carpeta que se pueda usar: se quita la clave, los accesos
        // directos y lo que haya en la carpeta por defecto
        log::warn!(
            "La clave de Transcriptor tiene InstallLocation «{location}»; uso la carpeta por defecto"
        );
        return Some(InnoInstall {
            dir: l.legacy_default_dir(),
        });
    }
    let dir = l.legacy_default_dir();
    dir.join("_internal")
        .is_dir()
        .then_some(InnoInstall { dir })
}

/// Quita Transcriptor: `Transcriptor.exe`, `unins000.exe/.dat/.msg` y `_internal\` (cada uno con 3
/// intentos: el proceso recién cerrado puede tardar en soltarlos), la carpeta si queda vacía, sus
/// tres accesos directos y su clave. Devuelve lo que no se pudo hacer.
pub fn remove_inno(l: &Layout, i: &InnoInstall) -> Vec<String> {
    let mut problems = Vec::new();
    let mut note = |r: Result<(), String>| {
        if let Err(e) = r {
            log::warn!("{e}");
            problems.push(e);
        }
    };
    note(retry(|| remove_file(&i.dir.join(LEGACY_EXE))));
    if let Ok(entries) = fs::read_dir(&i.dir) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_file())
                && is_inno_uninstaller(&e.file_name().to_string_lossy())
            {
                note(retry(|| remove_file(&e.path())));
            }
        }
    }
    note(remove_tree(l, &i.dir.join("_internal")));
    if fs::remove_dir(&i.dir).is_ok() {
        log::info!("Carpeta de Transcriptor borrada: {}", i.dir.display());
    }
    for lnk in [
        l.programs.join(LEGACY_LNK),
        l.desktop.join(LEGACY_LNK),
        l.startup.join(LEGACY_STARTUP_LNK),
    ] {
        note(remove_file(&lnk));
    }
    note(registry::delete_tree(&l.inno_key()));
    problems
}

/// `unins000.exe`, `unins000.dat`, `unins001.msg`…: lo que deja Inno para desinstalar.
fn is_inno_uninstaller(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let b = name.as_bytes();
    b.len() == 12
        && name.starts_with("unins")
        && b[5..8].iter().all(u8::is_ascii_digit)
        && [".exe", ".dat", ".msg"].contains(&&name[8..])
}

/// Borra la caché de CUDA de la app de Python (unos 2 GB que ya no sirven), y su carpeta si queda
/// vacía. Nunca si dentro están las grabaciones.
pub fn remove_cuda_cache(l: &Layout) -> Result<(), String> {
    let local = l.legacy_local_dir();
    remove_tree(l, &local.join("cuda"))?;
    let _ = fs::remove_dir(&local);
    Ok(())
}

/// Borra la carpeta con todo lo que tenga (3 intentos), salvo que guarde grabaciones: la carpeta
/// de grabaciones de Anotalo o la de la app de Python está dentro (o es ella).
fn remove_tree(l: &Layout, dir: &Path) -> Result<(), String> {
    if holds_recordings(l, dir) {
        return Err(format!(
            "No borro {}: contiene tus grabaciones",
            dir.display()
        ));
    }
    retry(|| remove_dir_all(dir))
}

/// `dir` es o contiene una de las carpetas de grabaciones configuradas.
pub(crate) fn holds_recordings(l: &Layout, dir: &Path) -> bool {
    contains_any(dir, &recordings_dirs(l))
}

/// Las carpetas de grabaciones configuradas: la de Anotalo y la de la app de Python (solo se leen
/// sus `config.json`; sin ellos, la de por defecto).
pub(crate) fn recordings_dirs(l: &Layout) -> Vec<PathBuf> {
    [l.data_dir(), l.legacy_data_dir()]
        .iter()
        .map(|d| read_output_dir(&d.join("config.json"), Profile::Installed))
        .collect()
}

/// `dir` es o contiene alguna de `recordings`, o guarda transcripciones aunque ningún ajuste lo
/// diga: entonces no se borra.
pub(crate) fn contains_any(dir: &Path, recordings: &[PathBuf]) -> bool {
    let dir_n = normalized(dir);
    recordings
        .iter()
        .any(|rec| normalized(rec).starts_with(&dir_n))
        || holds_transcripts(dir)
}

/// La ruta comparable: completa, sin `..`, con los nombres largos y los enlaces resueltos si
/// existe, sin el prefijo `\\?\` y en minúsculas.
fn normalized(p: &Path) -> PathBuf {
    let full = fs::canonicalize(p)
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf());
    let s = full.to_string_lossy().to_lowercase();
    let s = match s.strip_prefix(r"\\?\unc\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s.strip_prefix(r"\\?\").map_or(s.clone(), str::to_string),
    };
    PathBuf::from(s)
}

/// Hay transcripciones (`.md`) o una papelera de Anotalo en algún sitio dentro de `dir`. Las
/// carpetas de ajustes, modelos y cachés nunca las tienen.
fn holds_transcripts(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        match e.file_type() {
            Ok(t) if t.is_dir() => name == ".papelera" || holds_transcripts(&path),
            Ok(_) => name.ends_with(".md"),
            Err(_) => false,
        }
    })
}

/// Si Transcriptor arrancaba con Windows: su acceso directo de inicio; si no está, el `autostart`
/// de sus ajustes. None sin ninguno de los dos.
pub fn legacy_autostart(l: &Layout) -> Option<bool> {
    if l.startup.join(LEGACY_STARTUP_LNK).exists() {
        return Some(true);
    }
    let bytes = fs::read(l.legacy_data_dir().join("config.json")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    v.get("autostart")?.as_bool()
}

/// Si Transcriptor tenía acceso directo en el escritorio.
pub fn legacy_desktop(l: &Layout) -> bool {
    l.desktop.join(LEGACY_LNK).exists()
}

/// Tres intentos, con medio segundo entre ellos.
fn retry(mut f: impl FnMut() -> Result<(), String>) -> Result<(), String> {
    let mut left = 3;
    loop {
        left -= 1;
        match f() {
            Err(_) if left > 0 => std::thread::sleep(Duration::from_millis(500)),
            r => return r,
        }
    }
}

/// Borra un archivo; que no exista no es un error.
pub(crate) fn remove_file(p: &Path) -> Result<(), String> {
    match fs::remove_file(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            Err(format!("no se pudo borrar {} ({e})", p.display()))
        }
        _ => Ok(()),
    }
}

pub(crate) fn remove_dir_all(p: &Path) -> Result<(), String> {
    match fs::remove_dir_all(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            Err(format!("no se pudo borrar {} ({e})", p.display()))
        }
        _ => Ok(()),
    }
}

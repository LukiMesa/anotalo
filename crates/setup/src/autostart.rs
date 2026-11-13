//! Inicio con Windows: el acceso directo `Anotalo.lnk` en la carpeta Inicio del usuario, con
//! `--minimized` (port de `calltx/autostart.py`). Lo crea o lo quita el instalador, o el ajuste
//! "Iniciar con Windows"; la versión de desarrollo nunca lo toca.

use anotalo_core::config::Settings;
use anotalo_core::paths::Profile;
use anotalo_winsys::shortcut::{self, Shortcut};

use crate::layout::Layout;

/// Crea (o rehace) el acceso directo de inicio hacia el exe instalado.
pub fn enable(l: &Layout) -> Result<(), String> {
    shortcut::create(
        &l.startup_lnk(),
        &Shortcut {
            target: l.exe(),
            args: "--minimized".into(),
            workdir: l.install_dir(),
            description: "Anotalo".into(),
            aumid: None,
        },
    )
}

/// Quita el acceso directo de inicio (si no existe, no pasa nada).
pub fn disable(l: &Layout) -> Result<(), String> {
    let lnk = l.startup_lnk();
    match std::fs::remove_file(&lnk) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("no se pudo borrar {} ({e})", lnk.display())),
    }
}

pub fn is_enabled(l: &Layout) -> bool {
    l.startup_lnk().exists()
}

/// Versión instalada: el ajuste refleja siempre si existe el acceso directo (lo crea o quita el
/// instalador al instalar o actualizar, o el propio ajuste). En desarrollo nunca se toca nada.
/// Devuelve true si cambió `s` (hay que guardar).
pub fn ensure_autostart(s: &mut Settings, p: Profile, l: &Layout) -> bool {
    if p == Profile::Dev {
        return false;
    }
    let enabled = is_enabled(l);
    let changed = s.autostart != enabled || !s.autostart_configured;
    s.autostart = enabled;
    s.autostart_configured = true;
    changed
}

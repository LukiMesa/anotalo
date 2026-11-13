//! El exe es un programa de ventanas: no abre una consola al hacer doble clic ni al iniciar
//! Windows. Las órdenes de la línea de comandos escriben, aun así, en la terminal desde la que se
//! lanzan: cmd.exe no pasa sus identificadores a un programa de ventanas, así que hay que
//! engancharse a su consola.

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::os::windows::io::IntoRawHandle;

use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, GetHandleInformation, HANDLE};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetStdHandle,
};

/// ¿Falta este identificador estándar? Falta si es nulo o no vale en este proceso (un programa de
/// ventanas puede heredar el número de uno que no se le pasó). Uno redirigido (tubería, archivo,
/// NUL) no falta.
fn missing(which: STD_HANDLE) -> bool {
    // SAFETY: solo lee un identificador estándar de este proceso.
    match unsafe { GetStdHandle(which) } {
        Ok(h) if !h.0.is_null() && !h.is_invalid() => {
            let mut flags = 0u32;
            // SAFETY: consulta un identificador; si no es válido, falla sin más.
            unsafe { GetHandleInformation(h, &mut flags) }.is_err()
        }
        _ => true,
    }
}

/// Apunta un identificador estándar a la consola (`CONIN$` o `CONOUT$`). El identificador se
/// queda abierto mientras viva el proceso.
fn point(which: STD_HANDLE, device: &str) {
    match OpenOptions::new().read(true).write(true).open(device) {
        Ok(f) => {
            let h = HANDLE(f.into_raw_handle());
            // SAFETY: identificador propio, válido mientras viva el proceso.
            if let Err(e) = unsafe { SetStdHandle(which, h) } {
                log::debug!("No se pudo usar {device}: {e}");
            }
        }
        Err(e) => log::debug!("No se pudo abrir {device}: {e}"),
    }
}

/// Si la salida o los errores no van a ninguna parte, se engancha a la consola de quien lanzó el
/// exe (cmd, PowerShell) y le apunta la entrada, la salida y los errores que falten. Los
/// redirigidos (tuberías, archivos) nunca se tocan. Devuelve true si hay dónde escribir.
pub fn attach_parent() -> bool {
    let (input, output, error) = (
        missing(STD_INPUT_HANDLE),
        missing(STD_OUTPUT_HANDLE),
        missing(STD_ERROR_HANDLE),
    );
    if output || error {
        // SAFETY: sin punteros. Falla si quien lo lanzó no tiene consola (el Explorador, el
        // inicio de Windows); ERROR_ACCESS_DENIED = este proceso ya tiene una.
        let attached = match unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } {
            Ok(()) => true,
            Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
        };
        if attached {
            for (gone, which, device) in [
                (input, STD_INPUT_HANDLE, "CONIN$"),
                (output, STD_OUTPUT_HANDLE, "CONOUT$"),
                (error, STD_ERROR_HANDLE, "CONOUT$"),
            ] {
                if gone {
                    point(which, device);
                }
            }
        }
    }
    !missing(STD_OUTPUT_HANDLE) || !missing(STD_ERROR_HANDLE)
}

/// ¿La app de la bandeja escribe también en la terminal? Solo con `ANOTALO_CONSOLE=1` (para
/// desarrollo): enganchada a una consola, Windows la cerraría con un Ctrl+C en esa terminal o al
/// cerrarla, a mitad de una grabación.
pub fn tray_wants_console(var: Option<&OsStr>) -> bool {
    var.is_some_and(|v| v == "1")
}

/// ¿Hay a dónde escribir sin engancharse a nada? (salida o errores redirigidos)
pub fn has_output() -> bool {
    !missing(STD_OUTPUT_HANDLE) || !missing(STD_ERROR_HANDLE)
}

/// Para la app de la bandeja: se engancha a la terminal solo si se pide, y entonces ignora Ctrl+C
/// (cerrar la terminal sí la cierra: es solo para desarrollo). Devuelve si hay dónde escribir.
pub fn attach_for_tray() -> bool {
    if !tray_wants_console(std::env::var_os("ANOTALO_CONSOLE").as_deref()) {
        return false;
    }
    let attached = attach_parent();
    if attached {
        // SAFETY: sin manejador: este proceso deja de atender Ctrl+C.
        let _ = unsafe { SetConsoleCtrlHandler(None, true) };
    }
    attached
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La app de la bandeja no se engancha a la terminal desde la que se abre (Ctrl+C o cerrar esa
    /// terminal la cerrarían a mitad de una grabación), salvo que se pida con ANOTALO_CONSOLE=1.
    #[test]
    fn test_tray_stays_off_the_terminal_unless_asked() {
        assert!(!tray_wants_console(None));
        assert!(!tray_wants_console(Some(OsStr::new(""))));
        assert!(!tray_wants_console(Some(OsStr::new("0"))));
        assert!(tray_wants_console(Some(OsStr::new("1"))));
    }
}

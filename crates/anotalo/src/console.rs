//! El exe es un programa de ventanas: no abre una consola al hacer doble clic ni al iniciar
//! Windows. Las órdenes de la línea de comandos escriben, aun así, en la terminal desde la que se
//! lanzan: cmd.exe no pasa sus identificadores a un programa de ventanas, así que hay que
//! engancharse a su consola.

use std::fs::OpenOptions;
use std::os::windows::io::IntoRawHandle;

use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, GetHandleInformation, HANDLE};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
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

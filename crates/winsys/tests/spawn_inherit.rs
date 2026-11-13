//! Un programa lanzado con `spawn_detached` (el instalador relanza la app; la app lanza el
//! instalador) no se queda con la salida de quien lo lanza: si no, quien la lee (PowerShell con
//! `| Out-Host`) espera a que ese programa se cierre. Un solo test en su binario: cambia la salida
//! estándar del proceso un momento.
use std::io::Read;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anotalo_winsys::process;
use windows::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE, SetStdHandle};

#[test]
fn test_detached_child_does_not_keep_our_output() {
    let (mut reader, writer) = std::io::pipe().unwrap();
    let w = HANDLE(writer.as_raw_handle());
    // SAFETY: identificadores propios; la salida estándar se devuelve enseguida.
    let old = unsafe {
        SetHandleInformation(w, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT).unwrap();
        let old = GetStdHandle(STD_OUTPUT_HANDLE).unwrap();
        SetStdHandle(STD_OUTPUT_HANDLE, w).unwrap();
        old
    };
    let ping = PathBuf::from(r"C:\Windows\System32\PING.EXE");
    let spawned = process::spawn_detached(&ping, &["-n", "4", "127.0.0.1"], Path::new(r"C:\"));
    // SAFETY: la salida estándar que tenía el proceso.
    unsafe { SetStdHandle(STD_OUTPUT_HANDLE, old).unwrap() };
    let pid = spawned.unwrap();
    drop(writer);
    // sin nadie más con el extremo de escritura, la lectura acaba enseguida
    let t0 = Instant::now();
    let mut out = Vec::new();
    reader.read_to_end(&mut out).unwrap();
    let waited = t0.elapsed();
    let _ = process::terminate(pid);
    assert!(
        waited < Duration::from_secs(1),
        "la lectura esperó {waited:?}: el programa lanzado heredó nuestra salida"
    );
}

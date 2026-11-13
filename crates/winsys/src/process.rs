//! Otros procesos: listarlos, cerrarlos, esperar a que terminen y lanzar uno que sobreviva a quien
//! lo lanza (el instalador relanza la app; el desinstalador se borra con un `cmd.exe` oculto).

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS,
    SetHandleInformation, WAIT_OBJECT_0,
};
use windows::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};
use windows::core::PWSTR;

/// Un proceso en marcha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    /// Nombre del exe (`Anotalo.exe`).
    pub name: String,
    /// Ruta completa del exe (None si no se puede abrir el proceso, p. ej. uno del sistema).
    pub path: Option<PathBuf>,
}

/// Los procesos de este momento.
pub fn list() -> Vec<Proc> {
    let mut out = Vec::new();
    // SAFETY: instantánea propia, cerrada al final; estructura con su tamaño.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snap, &mut e).is_ok();
        while more {
            let n = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            out.push(Proc {
                pid: e.th32ProcessID,
                name: String::from_utf16_lossy(&e.szExeFile[..n]),
                path: image_path(e.th32ProcessID),
            });
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn image_path(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }
    // SAFETY: handle abierto y cerrado aquí; búfer del tamaño indicado.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }
}

/// Termina el proceso de golpe (como `taskkill /F`).
pub fn terminate(pid: u32) -> Result<(), String> {
    // SAFETY: handle abierto y cerrado aquí.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, false, pid)
            .map_err(|e| format!("no se pudo abrir el proceso {pid} ({e})"))?;
        let r = TerminateProcess(h, 1)
            .map_err(|e| format!("no se pudo terminar el proceso {pid} ({e})"));
        let _ = CloseHandle(h);
        r
    }
}

/// Espera a que terminen todos (true) o a que pase `timeout` (false).
pub fn wait_exit(pids: &[u32], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut handles: Vec<HANDLE> = Vec::new();
    let mut unopened: Vec<u32> = Vec::new();
    for &pid in pids {
        // SAFETY: los handles se cierran al final.
        match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
            Ok(h) => handles.push(h),
            // Ya terminó (o no se puede abrir): se comprueba en la lista de procesos.
            Err(_) => unopened.push(pid),
        }
    }
    let mut all = true;
    for &h in &handles {
        let left = deadline.saturating_duration_since(Instant::now());
        // SAFETY: handle abierto arriba.
        if unsafe { WaitForSingleObject(h, left.as_millis().min(u32::MAX as u128 - 1) as u32) }
            != WAIT_OBJECT_0
        {
            all = false;
            break;
        }
    }
    for h in handles {
        // SAFETY: handle propio, cerrado una vez.
        let _ = unsafe { CloseHandle(h) };
    }
    if !all {
        return false;
    }
    while !unopened.is_empty() {
        let alive: Vec<u32> = list().into_iter().map(|p| p.pid).collect();
        unopened.retain(|pid| alive.contains(pid));
        if unopened.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

/// Lanza `exe` sin consola ni vínculo con este proceso: sigue vivo cuando este termina, aunque
/// esté en un trabajo (job) que mata a sus hijos. Si el trabajo no deja salir de él, se lanza
/// dentro. Devuelve su pid.
pub fn spawn_detached(exe: &Path, args: &[&str], workdir: &Path) -> Result<u32, String> {
    keep_std_handles();
    let base = DETACHED_PROCESS.0 | CREATE_NEW_PROCESS_GROUP.0;
    let spawn = |flags: u32| {
        Command::new(exe)
            .args(args)
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    let child = match spawn(base | CREATE_BREAKAWAY_FROM_JOB.0) {
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED.0 as i32) => spawn(base),
        r => r,
    }
    .map_err(|e| format!("no se pudo lanzar {} ({e})", exe.display()))?;
    Ok(child.id())
}

/// Que nadie herede la entrada y la salida de este proceso. `Command` deja heredar todo lo que sea
/// heredable aunque reciba `Stdio::null()`: el programa lanzado "suelto" se quedaría con la
/// tubería de quien nos lanzó, que esperaría (PowerShell con `| Out-Host`) a que se cerrara.
fn keep_std_handles() {
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: identificadores estándar de este proceso; si no valen, falla sin más.
        unsafe {
            if let Ok(h) = GetStdHandle(which)
                && !h.is_invalid()
                && !h.0.is_null()
            {
                let _ = SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
            }
        }
    }
}

/// Lanza `exe` oculto (sin ventana) con la línea de argumentos tal cual (`cmd.exe /c "…"` necesita
/// sus propias comillas). Devuelve su pid.
pub fn spawn_hidden_raw(exe: &Path, raw_args: &str) -> Result<u32, String> {
    let child = Command::new(exe)
        .raw_arg(raw_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map_err(|e| format!("no se pudo lanzar {} ({e})", exe.display()))?;
    Ok(child.id())
}

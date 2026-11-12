//! Títulos de las ventanas visibles de unos programas (para el nombre del sitio en el navegador y
//! el título de la reunión).

use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::BOOL;

/// Títulos (no vacíos) de las ventanas visibles de nivel superior cuyos procesos se llaman como
/// alguno de `exes` (en minúsculas, p. ej. "ms-teams.exe").
pub fn visible_titles(exes: &[&str]) -> Vec<String> {
    let wanted: Vec<String> = exes.iter().map(|e| e.to_lowercase()).collect();
    let names = process_names();
    let mut found: Vec<(u32, String)> = Vec::new();
    // SAFETY: la retrollamada solo se usa durante EnumWindows y recibe un puntero a `found`.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM((&raw mut found) as isize));
    }
    found
        .into_iter()
        .filter(|(pid, _)| names.get(pid).is_some_and(|n| wanted.contains(n)))
        .map(|(_, title)| title)
        .collect()
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` es el `&mut Vec` de visible_titles, vivo durante EnumWindows.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return true.into();
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if title.trim().is_empty() {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let found = &mut *(lparam.0 as *mut Vec<(u32, String)>);
        found.push((pid, title));
    }
    true.into()
}

/// pid -> nombre del exe en minúsculas.
fn process_names() -> HashMap<u32, String> {
    let mut out = HashMap::new();
    // SAFETY: instantánea de ToolHelp recorrida y cerrada aquí.
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
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(260);
            out.insert(
                e.th32ProcessID,
                String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase(),
            );
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

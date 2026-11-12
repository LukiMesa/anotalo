//! El registro de privacidad del micrófono de Windows:
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone`.
//! Mientras una app usa el micro, su `LastUsedTimeStop` vale 0 (port de calltx/detector.py).

use std::collections::HashMap;

use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_QWORD, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use windows::core::{PCWSTR, PWSTR, w};

const CONSENT_KEY: PCWSTR = w!(
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone"
);

/// Clave de una app no empaquetada, como en la app de Python: "NonPackaged/" + su subclave (la
/// ruta del exe con '#' en lugar de '\').
pub fn nonpackaged_key(subkey: &str) -> String {
    format!("NonPackaged/{subkey}")
}

/// Clave de la app -> (LastUsedTimeStart, LastUsedTimeStop).
pub fn mic_entries() -> Result<HashMap<String, (u64, u64)>, String> {
    let mut out = HashMap::new();
    // SAFETY: claves abiertas y cerradas aquí; cadenas terminadas en NUL.
    unsafe {
        let root = open(HKEY_CURRENT_USER, CONSENT_KEY)
            .map_err(|e| format!("no se pudo abrir el registro del micrófono ({})", e.0))?;
        read(root, "", &mut out);
        let _ = RegCloseKey(root);
    }
    Ok(out)
}

unsafe fn open(parent: HKEY, sub: PCWSTR) -> Result<HKEY, WIN32_ERROR> {
    let mut key = HKEY::default();
    // SAFETY: ver mic_entries.
    let rc = unsafe { RegOpenKeyExW(parent, sub, None, KEY_READ, &mut key) };
    if rc == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(rc)
    }
}

unsafe fn read(key: HKEY, prefix: &str, out: &mut HashMap<String, (u64, u64)>) {
    let mut i = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        // SAFETY: búfer del tamaño indicado.
        let rc = unsafe {
            RegEnumKeyExW(
                key,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if rc != ERROR_SUCCESS {
            return;
        }
        i += 1;
        let sub = String::from_utf16_lossy(&name[..len as usize]);
        let wide: Vec<u16> = sub.encode_utf16().chain([0]).collect();
        if sub == "NonPackaged" && prefix.is_empty() {
            // SAFETY: ver mic_entries.
            unsafe {
                if let Ok(child) = open(key, PCWSTR(wide.as_ptr())) {
                    read(child, "NonPackaged/", out);
                    let _ = RegCloseKey(child);
                }
            }
            continue;
        }
        // SAFETY: ver mic_entries.
        let times = unsafe {
            (
                qword(key, &wide, w!("LastUsedTimeStart")),
                qword(key, &wide, w!("LastUsedTimeStop")),
            )
        };
        if let (Some(start), Some(stop)) = times {
            out.insert(format!("{prefix}{sub}"), (start, stop));
        }
    }
}

unsafe fn qword(key: HKEY, sub: &[u16], value: PCWSTR) -> Option<u64> {
    let mut v = 0u64;
    let mut size = size_of::<u64>() as u32;
    // SAFETY: destino de 8 bytes, tamaño indicado.
    let rc = unsafe {
        RegGetValueW(
            key,
            PCWSTR(sub.as_ptr()),
            value,
            RRF_RT_REG_QWORD,
            None,
            Some((&raw mut v).cast()),
            Some(&mut size),
        )
    };
    (rc == ERROR_SUCCESS).then_some(v)
}

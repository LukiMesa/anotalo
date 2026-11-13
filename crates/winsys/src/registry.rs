//! El registro de Windows para instalar y desinstalar: claves de `HKCU` (salvo `get_string_hklm`),
//! con rutas relativas como `Software\Microsoft\Windows\CurrentVersion\Uninstall\Anotalo`.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_DWORD,
    REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::HSTRING;

/// Un valor que se escribe: texto (`REG_SZ`) o número (`REG_DWORD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegValue<'a> {
    Str(&'a str),
    Dword(u32),
}

/// Escribe los valores en la clave de `HKCU`, que se crea si no existe.
pub fn set_values(key: &str, values: &[(&str, RegValue)]) -> Result<(), String> {
    let mut h = HKEY::default();
    // SAFETY: cadenas válidas durante la llamada; la clave se cierra abajo.
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut h,
            None,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(format!("no se pudo crear HKCU\\{key} ({})", describe(rc)));
    }
    let mut result = Ok(());
    for (name, value) in values {
        let rc = match value {
            RegValue::Str(s) => {
                let bytes: Vec<u8> = s
                    .encode_utf16()
                    .chain([0])
                    .flat_map(u16::to_le_bytes)
                    .collect();
                // SAFETY: clave abierta; búfer con el texto UTF-16 y su NUL final.
                unsafe { RegSetValueExW(h, &HSTRING::from(*name), None, REG_SZ, Some(&bytes)) }
            }
            RegValue::Dword(n) => {
                // SAFETY: clave abierta; 4 bytes.
                unsafe {
                    RegSetValueExW(
                        h,
                        &HSTRING::from(*name),
                        None,
                        REG_DWORD,
                        Some(&n.to_le_bytes()),
                    )
                }
            }
        };
        if rc != ERROR_SUCCESS {
            result = Err(format!(
                "no se pudo escribir {name} en HKCU\\{key} ({})",
                describe(rc)
            ));
            break;
        }
    }
    // SAFETY: clave propia, cerrada una vez.
    let _ = unsafe { RegCloseKey(h) };
    result
}

/// Un valor de texto de `HKCU` (None si no existe o no es texto).
pub fn get_string(key: &str, name: &str) -> Option<String> {
    get_string_in(HKEY_CURRENT_USER, key, name)
}

/// Un valor de texto de `HKLM`.
pub fn get_string_hklm(key: &str, name: &str) -> Option<String> {
    get_string_in(HKEY_LOCAL_MACHINE, key, name)
}

/// Un valor `REG_DWORD` de `HKCU`.
pub fn get_dword(key: &str, name: &str) -> Option<u32> {
    let mut n = 0u32;
    let mut size = 4u32;
    // SAFETY: búfer de 4 bytes, del tamaño indicado.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut n).cast()),
            Some(&mut size),
        )
    };
    (rc == ERROR_SUCCESS).then_some(n)
}

/// Si la clave de `HKCU` existe.
pub fn exists(key: &str) -> bool {
    let mut h = HKEY::default();
    // SAFETY: la clave abierta se cierra enseguida.
    unsafe {
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            None,
            KEY_READ,
            &mut h,
        ) != ERROR_SUCCESS
        {
            return false;
        }
        let _ = RegCloseKey(h);
    }
    true
}

/// Borra la clave de `HKCU` con todo lo que tenga. Que no exista no es un error. Por seguridad no
/// borra claves de primer nivel (`Software`) ni rutas vacías.
pub fn delete_tree(key: &str) -> Result<(), String> {
    let parts: Vec<&str> = key.split('\\').collect();
    if parts.len() < 2 || parts.iter().any(|p| p.trim().is_empty()) {
        return Err(format!("clave no válida para borrar: «{key}»"));
    }
    // SAFETY: nombre válido durante la llamada.
    let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) };
    if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("no se pudo borrar HKCU\\{key} ({})", describe(rc)))
    }
}

fn get_string_in(root: HKEY, key: &str, name: &str) -> Option<String> {
    let (key, name) = (HSTRING::from(key), HSTRING::from(name));
    // El valor puede cambiar entre la consulta del tamaño y la lectura: se reintenta.
    for _ in 0..3 {
        let mut size = 0u32;
        // SAFETY: solo pide el tamaño.
        let rc = unsafe {
            RegGetValueW(
                root,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: búfer del tamaño indicado en bytes.
        let rc = unsafe {
            RegGetValueW(
                root,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if rc == ERROR_SUCCESS {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            return Some(String::from_utf16_lossy(&buf[..len]));
        }
    }
    None
}

fn describe(rc: WIN32_ERROR) -> String {
    windows::core::Error::from(rc.to_hresult()).message()
}

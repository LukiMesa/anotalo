//! Avisos de Windows (WinRT `ToastNotificationManager`) con el AppUserModelID de Anotalo,
//! registrado en `HKCU\Software\Classes\AppUserModelId\Anotalo` (sin acceso directo). Al hacer clic
//! en "Transcripción guardada" se abre el archivo: activación por protocolo con `file:///…`, que no
//! necesita un servidor COM propio (un solo exe). Igual con una página web (`https://…`).

use std::fmt::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegSetValueExW,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::core::{HSTRING, PCWSTR, w};

pub const AUMID: &str = "Anotalo";

/// El icono de los avisos de `aumid` (el valor `IconUri` de su clave): la ruta de un PNG.
pub fn set_aumid_icon(aumid: &str, png: &Path) -> Result<(), String> {
    let path = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{aumid}"));
    let value: Vec<u16> = png.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: clave de HKCU abierta y cerrada aquí; el valor es REG_SZ terminado en NUL.
    unsafe {
        let mut key = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        );
        if rc != ERROR_SUCCESS {
            return Err(format!(
                "no se pudo registrar el icono de los avisos ({})",
                rc.0
            ));
        }
        let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
        let rc = RegSetValueExW(key, w!("IconUri"), None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        if rc != ERROR_SUCCESS {
            return Err(format!(
                "no se pudo registrar el icono de los avisos ({})",
                rc.0
            ));
        }
    }
    Ok(())
}

/// Registra el AppUserModelID con el nombre que se ve en los avisos.
pub fn register_aumid(aumid: &str, display_name: &str) -> Result<(), String> {
    let path = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{aumid}"));
    let value: Vec<u16> = display_name.encode_utf16().chain([0]).collect();
    // SAFETY: clave de HKCU abierta y cerrada aquí; el valor es REG_SZ terminado en NUL.
    unsafe {
        let mut key = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        );
        if rc != ERROR_SUCCESS {
            return Err(format!("no se pudo registrar el aviso ({})", rc.0));
        }
        let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
        let rc = RegSetValueExW(key, w!("DisplayName"), None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        if rc != ERROR_SUCCESS {
            return Err(format!("no se pudo registrar el aviso ({})", rc.0));
        }
    }
    Ok(())
}

pub fn set_process_aumid(aumid: &str) -> Result<(), String> {
    // SAFETY: cadena terminada en NUL válida durante la llamada.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(aumid)) }
        .map_err(|e| e.to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// `file:///C:/Llamadas/a%20b.md`: barras normales y todo lo que no sea seguro, en %XX (UTF-8).
fn file_uri(path: &Path) -> String {
    let mut out = String::from("file:///");
    for b in path.to_string_lossy().replace('\\', "/").bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Lo que se abre al pulsar el aviso (activación por protocolo).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    /// Un archivo, como `file:///…`.
    File(PathBuf),
    /// Una página web (`https://…`), tal cual.
    Url(String),
}

pub fn toast_xml(title: &str, body: &str, click: Option<&Click>) -> String {
    let launch = click.map_or(String::new(), |c| {
        let uri = match c {
            Click::File(p) => file_uri(p),
            Click::Url(u) => u.clone(),
        };
        format!(
            r#" activationType="protocol" launch="{}""#,
            xml_escape(&uri)
        )
    });
    format!(
        r#"<toast{launch}><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>"#,
        xml_escape(title),
        xml_escape(body)
    )
}

pub fn show(aumid: &str, title: &str, body: &str, click: Option<&Click>) -> Result<(), String> {
    // SAFETY: inicia WinRT en este hilo si aún no lo está (si ya lo está, no pasa nada).
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let doc = XmlDocument::new().map_err(|e| e.to_string())?;
    doc.LoadXml(&HSTRING::from(toast_xml(title, body, click)))
        .map_err(|e| e.to_string())?;
    let toast = ToastNotification::CreateToastNotification(&doc).map_err(|e| e.to_string())?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid))
        .and_then(|n| n.Show(&toast))
        .map_err(|e| e.to_string())
}

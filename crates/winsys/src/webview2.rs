//! ¿Está instalado Microsoft Edge WebView2 Runtime? La ventana de la app lo necesita. Se mira en el
//! registro como hacía el instalador de Inno (`packaging/installer.iss`).

use crate::registry;

/// La página de descarga de Microsoft (la misma que ofrecía el instalador de Inno).
pub const DOWNLOAD_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// La primera versión que no esté vacía ni sea `0.0.0.0` (lo que queda tras desinstalarlo).
pub fn pick_version(values: &[Option<String>]) -> Option<String> {
    values
        .iter()
        .flatten()
        .map(|v| v.trim())
        .find(|v| !v.is_empty() && *v != "0.0.0.0")
        .map(str::to_string)
}

/// La versión instalada (None si no lo está): el valor `pv` en HKLM (32 y 64 bits) y en HKCU.
pub fn installed_version() -> Option<String> {
    pick_version(&[
        registry::get_string_hklm(&format!(r"SOFTWARE\WOW6432Node\{CLIENT}"), "pv"),
        registry::get_string_hklm(&format!(r"SOFTWARE\{CLIENT}"), "pv"),
        registry::get_string(&format!(r"Software\{CLIENT}"), "pv"),
    ])
}

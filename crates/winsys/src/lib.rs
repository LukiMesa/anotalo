//! Servicios de Windows sin interfaz: señales para detectar llamadas, portapapeles, avisos,
//! instancia única y el cargador de WebView2; y, para instalar y desinstalar, registro, accesos
//! directos, procesos, cuadros de mensaje y la comprobación de WebView2.

pub mod clipboard;
pub mod consent;
pub mod dialog;
pub mod instance;
pub mod process;
pub mod registry;
pub mod shortcut;
pub mod toast;
pub mod webview2;
pub mod webview2_loader;
pub mod windows;

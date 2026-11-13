//! Servicios de Windows sin interfaz: señales para detectar llamadas, portapapeles, avisos e
//! instancia única; y, para instalar y desinstalar, registro, accesos directos, procesos, cuadros
//! de mensaje y WebView2.

pub mod clipboard;
pub mod consent;
pub mod dialog;
pub mod instance;
pub mod process;
pub mod registry;
pub mod shortcut;
pub mod toast;
pub mod webview2;
pub mod windows;

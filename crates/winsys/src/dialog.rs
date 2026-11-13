//! Cuadros de mensaje nativos (el instalador sin ventana, o sin WebView2), con el título "Anotalo",
//! siempre encima y al frente.

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
    MESSAGEBOX_STYLE, MessageBoxW, SW_SHOWNORMAL,
};
use windows::core::{HSTRING, w};

fn show(text: &str, style: MESSAGEBOX_STYLE) -> bool {
    // SAFETY: cadenas válidas durante la llamada; sin ventana dueña.
    let r = unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            w!("Anotalo"),
            style | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    r == IDYES
}

/// Pregunta Sí/No; true si la respuesta es Sí.
pub fn ask(text: &str) -> bool {
    show(text, MB_YESNO | MB_ICONQUESTION)
}

/// Avisa con un botón Aceptar.
pub fn tell(text: &str) {
    show(text, MB_OK | MB_ICONINFORMATION);
}

/// Abre la dirección en el navegador predeterminado.
pub fn open_url(url: &str) {
    // SAFETY: cadenas válidas durante la llamada.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(url),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

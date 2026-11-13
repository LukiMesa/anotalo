//! La ventana oculta de la app, en el hilo principal: recibe la bandeja, los atajos, los avisos de
//! la app y el cierre de sesión de Windows. Es de nivel superior (nunca visible) y no `HWND_MESSAGE`,
//! porque solo esas reciben `WM_ENDSESSION` y "TaskbarCreated" (el Explorador se reinició).

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostMessageW,
    RegisterClassW, RegisterWindowMessageW, TranslateMessage, WM_APP, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

/// Clase de la ventana (la busca el test de punta a punta).
pub const CLASS: PCWSTR = w!("AnotaloMessageWindow");
/// Hay avisos de la app en la cola.
pub const WM_APP_EVENT: u32 = WM_APP + 2;
/// La app terminó de cerrarse (tras "Salir").
pub const WM_APP_QUIT_DONE: u32 = WM_APP + 3;
/// Hay respuestas para la página.
pub const WM_APP_UI: u32 = WM_APP + 4;
/// Hay algo que hacer en el hilo principal (`MainCalls`).
pub const WM_APP_CALL: u32 = WM_APP + 5;
/// Otra Anotalo pidió mostrar la ventana.
pub const WM_APP_SHOW: u32 = WM_APP + 6;
/// La ventana principal cambió (wParam: 0 se ocultó, 1 maximizada, 2 restaurada).
pub const WM_APP_WINDOW: u32 = WM_APP + 7;

/// Quien atiende los mensajes. `None`: lo que Windows haga por defecto.
pub trait Handler {
    fn handle(&self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT>;
}

thread_local! {
    static HANDLER: RefCell<Option<Rc<dyn Handler>>> = RefCell::new(None);
}

pub fn set_handler(h: Option<Rc<dyn Handler>>) {
    HANDLER.with(|c| *c.borrow_mut() = h);
}

/// El mensaje que el Explorador manda al arrancar (hay que volver a poner el icono).
pub fn taskbar_created() -> u32 {
    // SAFETY: cadena estática.
    unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }
}

pub fn create() -> Result<HWND, String> {
    // SAFETY: clase y ventana de este proceso; `wndproc` vive lo que el proceso.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(windows::core::Error::from_thread().to_string());
        }
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS,
            w!("Anotalo"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())
    }
}

/// El bucle de mensajes del hilo, hasta `PostQuitMessage`.
pub fn run() {
    let mut msg = MSG::default();
    // SAFETY: bucle estándar sobre la cola de este hilo.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Desde cualquier hilo (la ventana se pasa como número: `HWND` no cruza hilos).
pub fn post(hwnd: isize, msg: u32, wp: usize) {
    // SAFETY: mensaje sin punteros.
    let _ = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(wp), LPARAM(0)) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // se clona y se suelta antes de atender: el menú de la bandeja vuelve a entrar aquí
    let handler = HANDLER.with(|c| c.borrow().clone());
    if let Some(h) = handler {
        match catch_unwind(AssertUnwindSafe(|| h.handle(hwnd, msg, wp, lp))) {
            Ok(Some(r)) => return r,
            Ok(None) => {}
            Err(_) => log::error!("Error atendiendo el mensaje {msg:#x}"),
        }
    }
    // SAFETY: procedimiento por defecto de Windows.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

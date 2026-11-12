//! Instancia única: un mutex con nombre. La segunda instancia avisa a la primera (un evento con
//! nombre) para que muestre su ventana, y termina.

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, INFINITE, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

/// Mientras viva, esta es la instancia de la app.
pub struct InstanceGuard(HANDLE);

// SAFETY: un HANDLE de mutex se puede cerrar desde cualquier hilo.
unsafe impl Send for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: handle propio, cerrado una vez.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// None si otra instancia ya tiene el mutex.
pub fn acquire(mutex: &str) -> Option<InstanceGuard> {
    // SAFETY: nombre válido durante la llamada; el handle se cierra en Drop.
    unsafe {
        let h = CreateMutexW(None, false, &HSTRING::from(mutex)).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return None;
        }
        Some(InstanceGuard(h))
    }
}

/// Pide a la primera instancia que se muestre.
pub fn signal_show(event: &str) {
    // SAFETY: handle abierto y cerrado aquí.
    unsafe {
        if let Ok(h) = CreateEventW(None, false, false, &HSTRING::from(event)) {
            let _ = SetEvent(h);
            let _ = CloseHandle(h);
        }
    }
}

/// Llama a `f` cada vez que otra instancia pide mostrarse (hilo propio, vive lo que la app).
pub fn on_show(event: &str, f: Box<dyn Fn() + Send>) {
    let name = HSTRING::from(event);
    let _ = std::thread::Builder::new()
        .name("mostrar".into())
        .spawn(move || {
            // SAFETY: el evento vive lo que este hilo.
            unsafe {
                let Ok(h) = CreateEventW(None, false, false, &name) else {
                    return;
                };
                while WaitForSingleObject(h, INFINITE) == WAIT_OBJECT_0 {
                    f();
                }
            }
        });
}

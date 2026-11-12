//! Portapapeles de Windows (texto). Otro programa puede tenerlo abierto un momento: se reintenta.

use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

const CF_UNICODETEXT: u32 = 13;

fn open() -> Result<(), String> {
    for _ in 0..10 {
        // SAFETY: sin ventana dueña; se cierra en `Opened::drop`.
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("el portapapeles está ocupado".into())
}

struct Opened;

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: lo abrió `open` en este hilo.
        let _ = unsafe { CloseClipboard() };
    }
}

pub fn set_text(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    open()?;
    let _opened = Opened;
    // SAFETY: memoria global del tamaño del texto; si el portapapeles no se la queda, se libera.
    unsafe {
        EmptyClipboard().map_err(|e| e.to_string())?;
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).map_err(|e| e.to_string())?;
        let ptr = GlobalLock(mem) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err("no se pudo reservar memoria para el portapapeles".into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(mem);
        if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))) {
            let _ = GlobalFree(Some(mem));
            return Err(e.to_string());
        }
    }
    Ok(())
}

pub fn get_text() -> Result<String, String> {
    open()?;
    let _opened = Opened;
    // SAFETY: el bloque del portapapeles es suyo; solo se lee mientras está bloqueado.
    unsafe {
        let h = GetClipboardData(CF_UNICODETEXT).map_err(|e| e.to_string())?;
        let mem = HGLOBAL(h.0);
        let ptr = GlobalLock(mem) as *const u16;
        if ptr.is_null() {
            return Err("el portapapeles no tiene texto".into());
        }
        let mut n = 0;
        while *ptr.add(n) != 0 {
            n += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, n));
        let _ = GlobalUnlock(mem);
        Ok(text)
    }
}

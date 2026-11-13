//! Atajos globales con `RegisterHotKey`, sobre la ventana de mensajes de la app (hilo principal):
//! grabar/parar (`hotkey`) y marcar (`hotkey_mark`). Un atajo que otra app ya usa no se pierde al
//! intentar cambiarlo: se queda el anterior.

use std::collections::HashMap;

use anotalo_core::hotkey::{parse_hotkey, validate_hotkey};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyId {
    Record = 1,
    Mark = 2,
}

pub struct Hotkeys {
    hwnd: HWND,
    /// id → (atajo, modificadores, tecla)
    specs: HashMap<HotkeyId, (String, u32, u32)>,
    /// Soltados mientras la ventana captura uno nuevo (`pause`).
    paused: bool,
}

impl Hotkeys {
    pub fn new(hwnd: HWND) -> Hotkeys {
        Hotkeys {
            hwnd,
            specs: HashMap::new(),
            paused: false,
        }
    }

    #[allow(dead_code)] // M3: la ventana de ajustes cambia los atajos
    pub fn spec(&self, id: HotkeyId) -> Option<&str> {
        self.specs.get(&id).map(|(s, _, _)| s.as_str())
    }

    /// Registra un atajo. Si otra app lo usa, Err (y se recuerda para `resume`).
    pub fn register(&mut self, id: HotkeyId, spec: &str) -> Result<(), String> {
        let (mods, vk) = parse_hotkey(spec)?;
        self.specs.insert(id, (spec.to_string(), mods, vk));
        if self.try_register(id, mods, vk) {
            Ok(())
        } else {
            log::error!("No se pudo registrar el atajo {spec} (¿lo usa otra app?)");
            Err(format!("{spec} lo usa otra app"))
        }
    }

    /// Cambia un atajo en vivo. Si el nuevo no se puede registrar, se queda el anterior. En pausa
    /// (la ventana cambia el atajo mientras lo captura) solo se comprueba que esté libre: se
    /// registra al volver con `resume`.
    pub fn rebind(&mut self, id: HotkeyId, spec: &str) -> Result<(), String> {
        let (mods, vk) = validate_hotkey(spec)?;
        let old = self.specs.get(&id).cloned();
        self.unregister(id);
        if self.try_register(id, mods, vk) {
            self.specs.insert(id, (spec.to_string(), mods, vk));
            if self.paused {
                self.unregister(id);
            }
            return Ok(());
        }
        if let Some((_, m, v)) = old
            && !self.paused
        {
            self.try_register(id, m, v);
        }
        Err("Esa combinación ya la usa otra aplicación".into())
    }

    /// Suelta todos los atajos (p. ej. mientras la ventana captura uno nuevo).
    pub fn pause(&mut self) {
        self.paused = true;
        let ids: Vec<HotkeyId> = self.specs.keys().copied().collect();
        for id in ids {
            self.unregister(id);
        }
    }

    /// Vuelve a registrar los atajos soltados con `pause`.
    pub fn resume(&mut self) {
        if !std::mem::replace(&mut self.paused, false) {
            return;
        }
        let all: Vec<(HotkeyId, u32, u32)> = self
            .specs
            .iter()
            .map(|(id, (_, m, v))| (*id, *m, *v))
            .collect();
        for (id, m, v) in all {
            if !self.try_register(id, m, v) {
                log::warn!("No se pudo volver a registrar un atajo");
            }
        }
    }

    fn try_register(&self, id: HotkeyId, mods: u32, vk: u32) -> bool {
        // SAFETY: la ventana es de este hilo (la de mensajes de la app).
        unsafe { RegisterHotKey(Some(self.hwnd), id as i32, HOT_KEY_MODIFIERS(mods), vk).is_ok() }
    }

    fn unregister(&self, id: HotkeyId) {
        // SAFETY: ver try_register; soltar uno que no está registrado no hace nada.
        let _ = unsafe { UnregisterHotKey(Some(self.hwnd), id as i32) };
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.pause();
    }
}

#[cfg(test)]
mod tests {
    //! Necesitan una sesión de escritorio: `cargo test --release -p anotalo --bin anotalo -- --ignored`
    use super::*;
    use std::sync::mpsc;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
    };
    use windows::core::w;

    fn message_window() -> HWND {
        // SAFETY: ventana de solo mensajes de la clase predefinida STATIC, destruida al acabar.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .expect("ventana de mensajes")
        }
    }

    /// ¿Puede otro hilo registrar este atajo ahora mismo? (si sí, lo suelta enseguida)
    fn free_elsewhere(spec: &str) -> bool {
        let (mods, vk) = parse_hotkey(spec).unwrap();
        std::thread::spawn(move || {
            // SAFETY: registro asociado al hilo (sin ventana) y liberado aquí.
            unsafe {
                let ok = RegisterHotKey(None, 9, HOT_KEY_MODIFIERS(mods), vk).is_ok();
                if ok {
                    let _ = UnregisterHotKey(None, 9);
                }
                ok
            }
        })
        .join()
        .unwrap()
    }

    #[test]
    #[ignore]
    fn test_rebind_conflict_keeps_old() {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let (mods, vk) = parse_hotkey("<ctrl>+<alt>+<f12>").unwrap();
            // SAFETY: registro del hilo, liberado al final.
            unsafe {
                ready_tx
                    .send(RegisterHotKey(None, 7, HOT_KEY_MODIFIERS(mods), vk).is_ok())
                    .unwrap();
                let _ = release_rx.recv();
                let _ = UnregisterHotKey(None, 7);
            }
        });
        assert!(ready_rx.recv().unwrap(), "otra app tiene Ctrl+Alt+F12");
        let hwnd = message_window();
        let mut hk = Hotkeys::new(hwnd);
        hk.register(HotkeyId::Record, "<ctrl>+<alt>+<f11>").unwrap();
        let err = hk
            .rebind(HotkeyId::Record, "<ctrl>+<alt>+<f12>")
            .unwrap_err();
        assert_eq!(err, "Esa combinación ya la usa otra aplicación");
        assert_eq!(hk.spec(HotkeyId::Record), Some("<ctrl>+<alt>+<f11>"));
        assert!(
            !free_elsewhere("<ctrl>+<alt>+<f11>"),
            "el anterior sigue registrado"
        );
        drop(hk);
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        // SAFETY: la ventana es de este hilo.
        unsafe { DestroyWindow(hwnd).unwrap() };
    }

    #[test]
    #[ignore]
    fn test_rebind_success() {
        let hwnd = message_window();
        let mut hk = Hotkeys::new(hwnd);
        hk.register(HotkeyId::Mark, "<ctrl>+<alt>+<f10>").unwrap();
        hk.rebind(HotkeyId::Mark, "<ctrl>+<alt>+<f9>").unwrap();
        assert_eq!(hk.spec(HotkeyId::Mark), Some("<ctrl>+<alt>+<f9>"));
        assert!(
            free_elsewhere("<ctrl>+<alt>+<f10>"),
            "el anterior se suelta"
        );
        assert!(!free_elsewhere("<ctrl>+<alt>+<f9>"));
        drop(hk);
        assert!(
            free_elsewhere("<ctrl>+<alt>+<f9>"),
            "al soltar Hotkeys se liberan"
        );
        // SAFETY: la ventana es de este hilo.
        unsafe { DestroyWindow(hwnd).unwrap() };
    }

    #[test]
    #[ignore]
    fn test_pause_releases_and_resume_restores() {
        let spec = "<ctrl>+<alt>+<f8>";
        let hwnd = message_window();
        let mut hk = Hotkeys::new(hwnd);
        hk.register(HotkeyId::Record, spec).unwrap();
        assert!(!free_elsewhere(spec), "lo tiene la app");
        hk.pause();
        assert!(free_elsewhere(spec), "liberado durante la captura");
        hk.resume();
        assert!(!free_elsewhere(spec), "vuelve a estar registrado");
        drop(hk);
        // SAFETY: la ventana es de este hilo.
        unsafe { DestroyWindow(hwnd).unwrap() };
    }

    /// La ventana cambia el atajo mientras lo captura (con los atajos en pausa): el nuevo no se
    /// activa hasta que la captura termina.
    #[test]
    #[ignore]
    fn test_rebind_while_paused_waits_for_resume() {
        let (old, new) = ("<ctrl>+<alt>+<f7>", "<ctrl>+<alt>+<f6>");
        let hwnd = message_window();
        let mut hk = Hotkeys::new(hwnd);
        hk.register(HotkeyId::Record, old).unwrap();
        hk.pause();
        hk.rebind(HotkeyId::Record, new).unwrap();
        assert!(free_elsewhere(new), "sigue en pausa");
        assert!(free_elsewhere(old));
        hk.resume();
        assert!(!free_elsewhere(new), "el nuevo, al volver");
        assert!(free_elsewhere(old), "el anterior ya no");
        hk.resume(); // sin pausa no hace nada
        drop(hk);
        // SAFETY: la ventana es de este hilo.
        unsafe { DestroyWindow(hwnd).unwrap() };
    }

    #[test]
    fn test_register_rejects_a_bad_spec_without_touching_windows() {
        let mut hk = Hotkeys::new(HWND::default());
        assert_eq!(
            hk.register(HotkeyId::Record, "<ctrl>+<alt>"),
            Err("El atajo '<ctrl>+<alt>' no tiene tecla principal".to_string())
        );
        assert_eq!(
            hk.rebind(HotkeyId::Record, "<shift>+r"),
            Err("El atajo necesita Ctrl, Alt o Win".to_string())
        );
    }
}

//! El indicador de grabación (port de calltx/ui/indicator.py): una píldora abajo al centro, siempre
//! encima, que no roba el foco y deja pasar los clics. Es una ventana Win32 "layered" con
//! transparencia por píxel, en su propio hilo: 30 fps mientras se ve, dormido mientras no.
//!
//! - `PillModel`: qué se ve en cada momento (entrada, salida, "Marcado" 1,2 s…), sin Windows.
//! - `IndicatorWiring`: qué mostrar según los avisos de la app.
//! - `Indicator`: la ventana.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use anotalo_core::app::AppEvent;
use tiny_skia::Pixmap;
use windows::Win32::Foundation::{
    COLORREF, CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WAIT_FAILED, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HGDIOBJ,
    ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HWND_TOPMOST, MSG,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
    SPI_GETCLIENTAREAANIMATION, SPI_GETWORKAREA, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetWindowPos, ShowWindow,
    SystemParametersInfoW, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WINDOW_EX_STYLE,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::pill::{EXIT_S, PillState, enter_frame, exit_alpha, render_pill};

/// Estilos de la ventana: nunca se activa ni recibe clics.
pub const EX_STYLE: WINDOW_EX_STYLE = WINDOW_EX_STYLE(
    WS_EX_LAYERED.0
        | WS_EX_TRANSPARENT.0
        | WS_EX_TOPMOST.0
        | WS_EX_TOOLWINDOW.0
        | WS_EX_NOACTIVATE.0,
);

const FPS: u32 = 30;
/// "Copiado"/"Transcripción guardada" se ven 1,5 s.
const DONE_S: f64 = 1.5;
/// "Marcado" se ve 1,2 s y vuelve la grabación.
const MARKED_S: f64 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Recording,
    Transcribing,
    Done { copied: bool },
    Marked,
    Hide,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub state: PillState,
    pub alpha: u8,
    /// Desplazamiento hacia abajo (sin escalar) de la animación de entrada.
    pub dy: i32,
    /// Segundos de grabación.
    pub elapsed_s: f64,
}

/// Qué se ve en cada momento. `now` en segundos.
#[derive(Default)]
pub struct PillModel {
    state: Option<PillState>,
    started: f64,
    done_at: f64,
    shown_at: f64,
    leaving_at: Option<f64>,
    marked_until: f64,
    visible: bool,
}

impl PillModel {
    pub fn apply(&mut self, cmd: Cmd, now: f64) {
        if cmd == Cmd::Hide {
            if self.visible && self.state.is_some() && self.leaving_at.is_none() {
                self.leaving_at = Some(now); // se desvanece con lo último que mostraba
            } else if !self.visible {
                self.state = None;
            }
            return;
        }
        if cmd == Cmd::Marked {
            if self.state == Some(PillState::Recording) {
                self.marked_until = now + MARKED_S;
            }
            return;
        }
        self.leaving_at = None;
        match cmd {
            Cmd::Recording => {
                self.started = now;
                self.marked_until = 0.0;
                if !self.visible {
                    self.shown_at = now;
                }
                self.state = Some(PillState::Recording);
            }
            Cmd::Transcribing => self.state = Some(PillState::Transcribing),
            Cmd::Done { copied } => {
                self.done_at = now;
                self.state = Some(if copied {
                    PillState::Copied
                } else {
                    PillState::Saved
                });
            }
            Cmd::Marked | Cmd::Hide => {}
        }
    }

    /// El fotograma de ahora, o None si la píldora no se ve.
    pub fn frame(&mut self, now: f64, animate: bool) -> Option<Frame> {
        let done = matches!(self.state, Some(PillState::Copied | PillState::Saved));
        if done && self.leaving_at.is_none() && now - self.done_at > DONE_S {
            self.leaving_at = Some(self.done_at + DONE_S);
        }
        if self.state.is_some() && !self.visible && now - self.shown_at > super::pill::ENTER_S {
            self.shown_at = now;
        }
        let (mut alpha, mut dy) = (255u8, 0i32);
        if animate {
            (alpha, dy) = enter_frame(now - self.shown_at);
            if let Some(l) = self.leaving_at {
                alpha = alpha.min(exit_alpha(now - l));
            }
        }
        if let Some(l) = self.leaving_at
            && (!animate || now - l >= EXIT_S)
        {
            self.state = None;
            self.leaving_at = None;
        }
        let Some(mut state) = self.state else {
            self.visible = false;
            return None;
        };
        self.visible = true;
        if state == PillState::Recording && now < self.marked_until {
            state = PillState::Marked;
        }
        Some(Frame {
            state,
            alpha,
            dy,
            elapsed_s: now - self.started,
        })
    }
}

/// Lo que la app puede pedirle al indicador (la ventana real, o una falsa en los tests).
pub trait Pill: Send + Sync {
    fn show_recording(&self);
    fn push_level(&self, rms: f32);
    fn show_transcribing(&self);
    fn show_done(&self, copied: bool);
    fn flash_marked(&self);
    fn hide(&self);
}

/// Conecta los avisos de la app con el indicador. Al terminar una grabación la píldora siempre se
/// cierra, aunque se haya desactivado el indicador a mitad de grabación.
pub struct IndicatorWiring {
    enabled: AtomicBool,
    shown: AtomicBool,
    pill: Arc<dyn Pill>,
}

impl IndicatorWiring {
    pub fn new(enabled: bool, pill: Arc<dyn Pill>) -> IndicatorWiring {
        IndicatorWiring {
            enabled: AtomicBool::new(enabled),
            shown: AtomicBool::new(false),
            pill,
        }
    }

    /// El ajuste "Mostrar indicador" cambió.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
        if !on {
            self.pill.hide();
            self.shown.store(false, Ordering::SeqCst);
        }
    }

    pub fn on_event(&self, e: &AppEvent) {
        let enabled = self.enabled.load(Ordering::SeqCst);
        let shown = self.shown.load(Ordering::SeqCst);
        match e {
            AppEvent::SessionStarted { .. } if enabled => {
                self.pill.show_recording();
                self.shown.store(true, Ordering::SeqCst);
            }
            AppEvent::Level(v) => self.pill.push_level(*v),
            AppEvent::Marked { .. } if shown && enabled => self.pill.flash_marked(),
            AppEvent::SessionStopping { .. } if shown && enabled => self.pill.show_transcribing(),
            AppEvent::SessionEnded { path, copied, .. } => {
                if shown && enabled && path.is_some() {
                    self.pill.show_done(*copied);
                } else {
                    self.pill.hide();
                }
                self.shown.store(false, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}

type Render = fn(PillState, &[f32], f64, f32, f32) -> Pixmap;

enum Msg {
    Cmd(Cmd),
    Quit,
}

/// Despierta al hilo del indicador cuando hay una orden: oculto, duerme hasta entonces.
struct Wake(HANDLE);

// SAFETY: un evento de Windows se puede señalar y esperar desde cualquier hilo.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}

impl Drop for Wake {
    fn drop(&mut self) {
        // SAFETY: evento propio, cerrado una vez (al soltar el último `Arc`).
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// La ventana del indicador en su hilo.
pub struct Indicator {
    tx: Sender<Msg>,
    /// None si no se pudo crear el evento: entonces el hilo mira las órdenes 30 veces por segundo.
    wake: Option<Arc<Wake>>,
    levels: Arc<Mutex<VecDeque<f32>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Indicator {
    pub fn spawn() -> Indicator {
        Indicator::spawn_with(render_pill, Arc::default())
    }

    /// `wakes`: vueltas del bucle de la ventana (para los tests).
    fn spawn_with(render: Render, wakes: Arc<AtomicUsize>) -> Indicator {
        let (tx, rx) = mpsc::channel();
        let levels = Arc::new(Mutex::new(VecDeque::from(vec![0.0; 6])));
        // SAFETY: evento sin nombre, de reinicio automático; lo cierra `Wake`.
        let wake = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .ok()
            .map(|h| Arc::new(Wake(h)));
        let (lv, wk) = (levels.clone(), wake.clone());
        let thread = std::thread::Builder::new()
            .name("indicador".into())
            .spawn(move || window_loop(&rx, &lv, wk.as_deref(), render, &wakes))
            .ok();
        Indicator {
            tx,
            wake,
            levels,
            thread: Mutex::new(thread),
        }
    }

    fn post(&self, m: Msg) {
        let _ = self.tx.send(m);
        if let Some(w) = &self.wake {
            // SAFETY: evento propio, vivo mientras viva `self`.
            let _ = unsafe { SetEvent(w.0) };
        }
    }

    fn send(&self, c: Cmd) {
        self.post(Msg::Cmd(c));
    }

    /// Cierra la ventana y espera a su hilo (aunque otros aún tengan el indicador: lo que le
    /// pidan después no hace nada).
    pub fn close(&self) {
        self.post(Msg::Quit);
        let thread = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = thread {
            let _ = t.join();
        }
    }

    #[cfg(test)]
    fn alive(&self) -> bool {
        self.thread
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }
}

impl Pill for Indicator {
    fn show_recording(&self) {
        self.send(Cmd::Recording);
    }
    fn push_level(&self, rms: f32) {
        let mut l = self.levels.lock().unwrap_or_else(|e| e.into_inner());
        l.push_back((rms * 12.0).min(1.0));
        while l.len() > 6 {
            l.pop_front();
        }
    }
    fn show_transcribing(&self) {
        self.send(Cmd::Transcribing);
    }
    fn show_done(&self, copied: bool) {
        self.send(Cmd::Done { copied });
    }
    fn flash_marked(&self) {
        self.send(Cmd::Marked);
    }
    fn hide(&self) {
        self.send(Cmd::Hide);
    }
}

/// Al salir: la píldora se oculta y la ventana del indicador se cierra (el cableado aún la tiene).
pub fn shut_down(ind: Arc<Indicator>, wiring: &IndicatorWiring) {
    wiring.set_enabled(false);
    ind.close();
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: procedimiento por defecto de Windows.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Ajuste de Windows "Efectos de animación".
fn animations_enabled() -> bool {
    let mut v: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION escribe un BOOL en `v`.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut v).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || v != 0
}

/// Mientras se ve la píldora, 30 fotogramas por segundo; oculta, el hilo duerme hasta que llega una
/// orden (`wake`) o un mensaje de la ventana.
fn window_loop(
    rx: &Receiver<Msg>,
    levels: &Mutex<VecDeque<f32>>,
    wake: Option<&Wake>,
    render: Render,
    wakes: &AtomicUsize,
) {
    // SAFETY: ventana de este hilo; se destruye al salir.
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("AnotaloIndicator");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            EX_STYLE,
            class,
            w!("Anotalo"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            log::error!("No se pudo crear la ventana del indicador");
            return;
        };
        let dpi = GetDpiForWindow(hwnd);
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        let t0 = Instant::now();
        let mut model = PillModel::default();
        let mut shown = [0.0f32; 6];
        let mut visible = false;
        // la píldora tiene algo que mostrar (aunque un fotograma fallido no la haya pintado)
        let mut animating = false;
        let mut last = 0.0f64;
        let mut msg = MSG::default();
        let handles = wake.map(|w| [w.0]);
        loop {
            let frame_ms = 1000 / FPS;
            let timeout = if animating || handles.is_none() {
                frame_ms
            } else {
                INFINITE
            };
            let r = MsgWaitForMultipleObjects(
                handles.as_ref().map(|h| h.as_slice()),
                false,
                timeout,
                QS_ALLINPUT,
            );
            if r == WAIT_FAILED {
                std::thread::sleep(std::time::Duration::from_millis(u64::from(frame_ms)));
            }
            wakes.fetch_add(1, Ordering::Relaxed);
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let now = t0.elapsed().as_secs_f64();
            let dt = (now - last) as f32;
            last = now;
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Quit => {
                        let _ = DestroyWindow(hwnd);
                        return;
                    }
                    Msg::Cmd(c) => {
                        if c == Cmd::Recording {
                            let mut l = levels.lock().unwrap_or_else(|e| e.into_inner());
                            l.iter_mut().for_each(|v| *v = 0.0);
                        }
                        model.apply(c, now);
                    }
                }
            }
            let Some(f) = model.frame(now, animations_enabled()) else {
                animating = false;
                if visible {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    visible = false;
                }
                continue;
            };
            animating = true;
            // barras: suben rápido, bajan en ~120 ms
            let target: Vec<f32> = levels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .copied()
                .collect();
            for (s, t) in shown.iter_mut().zip(target) {
                *s = t.max(*s - dt / 0.12);
            }
            let img = catch_unwind(AssertUnwindSafe(|| {
                render(f.state, &shown, f.elapsed_s, scale, (now * 1.2) as f32)
            }));
            let Ok(img) = img else {
                log::error!("Error dibujando el indicador"); // un fotograma fallido no lo apaga
                continue;
            };
            blit(
                hwnd,
                &img,
                f.alpha,
                (f.dy as f32 * scale).round() as i32,
                scale,
            );
            if !visible {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                );
                visible = true;
            }
        }
    }
}

/// Pinta la píldora en la ventana, abajo al centro del área de trabajo.
unsafe fn blit(hwnd: HWND, img: &Pixmap, alpha: u8, dy: i32, scale: f32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    // SAFETY: recursos GDI creados y liberados aquí; el DIB tiene w×h×4 bytes.
    unsafe {
        let mut area = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&raw mut area).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let x = (area.left + area.right - w) / 2;
        let y = area.bottom - h - (16.0 * scale).round() as i32 + dy;
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // de arriba abajo
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            && !bits.is_null()
        {
            // tiny-skia: RGBA premultiplicado → Windows: BGRA premultiplicado
            let dst = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (w * h * 4) as usize);
            let (dst4, _) = dst.as_chunks_mut::<4>();
            let (src4, _) = img.data().as_chunks::<4>();
            for (d, s) in dst4.iter_mut().zip(src4) {
                *d = [s[2], s[1], s[0], s[3]];
            }
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                Some(&POINT { x, y }),
                Some(&SIZE { cx: w, cy: h }),
                Some(mem),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct FakePill(Mutex<Vec<String>>);

    impl FakePill {
        fn calls(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
        fn push(&self, s: &str) {
            self.0.lock().unwrap().push(s.into());
        }
    }

    impl Pill for FakePill {
        fn show_recording(&self) {
            self.push("show_recording");
        }
        fn push_level(&self, _rms: f32) {
            self.push("push_level");
        }
        fn show_transcribing(&self) {
            self.push("show_transcribing");
        }
        fn show_done(&self, copied: bool) {
            self.push(if copied {
                "show_done(copied)"
            } else {
                "show_done"
            });
        }
        fn flash_marked(&self) {
            self.push("flash_marked");
        }
        fn hide(&self) {
            self.push("hide");
        }
    }

    fn started(label: &str) -> AppEvent {
        AppEvent::SessionStarted {
            label: label.into(),
        }
    }

    fn ended(path: Option<&str>, label: &str, copied: bool) -> AppEvent {
        AppEvent::SessionEnded {
            path: path.map(PathBuf::from),
            label: label.into(),
            copied,
        }
    }

    #[test]
    fn test_end_always_clears_pill_even_if_disabled_midway() {
        let pill = Arc::new(FakePill::default());
        let w = IndicatorWiring::new(true, pill.clone());
        w.on_event(&started("Nota"));
        w.set_enabled(false);
        w.on_event(&AppEvent::SessionStopping {
            label: "Nota".into(),
        });
        w.on_event(&ended(None, "Nota", true));
        let calls = pill.calls();
        assert_eq!(calls[0], "show_recording");
        assert!(calls.contains(&"hide".to_string()));
        assert!(!calls.contains(&"show_transcribing".to_string()));
    }

    #[test]
    fn test_disabling_hides_at_once() {
        let pill = Arc::new(FakePill::default());
        let w = IndicatorWiring::new(true, pill.clone());
        w.on_event(&started("Nota"));
        w.set_enabled(false);
        assert_eq!(pill.calls(), vec!["show_recording", "hide"]);
    }

    #[test]
    fn test_failed_save_hides_instead_of_saved() {
        let pill = Arc::new(FakePill::default());
        let w = IndicatorWiring::new(true, pill.clone());
        w.on_event(&started("Teams"));
        w.on_event(&ended(None, "Teams", false));
        assert_eq!(pill.calls().last().map(String::as_str), Some("hide"));
    }

    #[test]
    fn test_marked_and_saved_flow() {
        let pill = Arc::new(FakePill::default());
        let w = IndicatorWiring::new(true, pill.clone());
        w.on_event(&started("Teams"));
        w.on_event(&AppEvent::Marked {
            hms: "00:01:00".into(),
        });
        w.on_event(&AppEvent::SessionStopping {
            label: "Teams".into(),
        });
        w.on_event(&ended(Some("x.md"), "Teams", false));
        assert_eq!(
            pill.calls(),
            vec![
                "show_recording",
                "flash_marked",
                "show_transcribing",
                "show_done"
            ]
        );
    }

    #[test]
    fn test_marked_flash_returns_to_recording() {
        let mut m = PillModel::default();
        m.apply(Cmd::Recording, 0.0);
        assert_eq!(m.frame(0.5, true).unwrap().state, PillState::Recording);
        m.apply(Cmd::Marked, 1.0);
        assert_eq!(m.frame(1.5, true).unwrap().state, PillState::Marked);
        let back = m.frame(2.3, true).unwrap();
        assert_eq!(back.state, PillState::Recording);
        assert!((back.elapsed_s - 2.3).abs() < 1e-9, "el reloj sigue");
    }

    #[test]
    fn test_done_fades_out_after_its_time() {
        let mut m = PillModel::default();
        m.apply(Cmd::Recording, 0.0);
        m.frame(0.1, true);
        m.apply(Cmd::Done { copied: true }, 1.0);
        assert_eq!(m.frame(2.0, true).unwrap().state, PillState::Copied);
        assert!(m.frame(2.55, true).unwrap().alpha < 255, "desvaneciéndose");
        assert!(m.frame(2.7, true).is_none());
    }

    #[test]
    fn test_window_styles_never_activate() {
        for flag in [
            WS_EX_NOACTIVATE,
            WS_EX_TRANSPARENT,
            WS_EX_TOPMOST,
            WS_EX_LAYERED,
        ] {
            assert_eq!(EX_STYLE.0 & flag.0, flag.0);
        }
    }

    static RENDERS: AtomicUsize = AtomicUsize::new(0);

    fn flaky(st: PillState, l: &[f32], e: f64, s: f32, sp: f32) -> Pixmap {
        if RENDERS.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("boom");
        }
        render_pill(st, l, e, s, sp)
    }

    #[test]
    #[ignore]
    fn test_render_error_does_not_kill_indicator() {
        let ind = Indicator::spawn_with(flaky, Arc::default());
        ind.show_recording();
        std::thread::sleep(std::time::Duration::from_millis(400));
        let alive = ind.alive();
        ind.close();
        assert!(alive);
        assert!(RENDERS.load(Ordering::SeqCst) > 1);
    }

    /// Al salir se cierra la ventana aunque el cableado (los avisos de la app) aún la tenga.
    /// Crea una ventana (oculta): `cargo test --release -p anotalo -- --ignored indicator`
    #[test]
    #[ignore]
    fn test_shut_down_closes_the_window_held_by_the_wiring() {
        let ind = Arc::new(Indicator::spawn());
        let wiring = IndicatorWiring::new(true, ind.clone() as Arc<dyn Pill>);
        let watch = Arc::downgrade(&ind);
        assert!(ind.alive());
        shut_down(ind, &wiring);
        assert!(
            !watch.upgrade().is_some_and(|i| i.alive()),
            "la ventana del indicador sigue abierta"
        );
    }

    /// Oculto, el hilo duerme hasta que llega una orden (no 30 veces por segundo todo el día).
    #[test]
    #[ignore]
    fn test_hidden_indicator_sleeps_until_a_command() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let ind = Indicator::spawn_with(render_pill, wakes.clone());
        std::thread::sleep(std::time::Duration::from_millis(600));
        let idle = wakes.load(Ordering::SeqCst);
        assert!(idle <= 3, "{idle} vueltas en 0,6 s sin nada que mostrar");
        ind.hide();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let after = wakes.load(Ordering::SeqCst);
        assert!(after > idle, "una orden lo despierta");
        assert!(after <= idle + 3, "y vuelve a dormir: {after}");
        ind.close();
    }
}

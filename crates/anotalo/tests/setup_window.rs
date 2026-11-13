//! La ventana del instalador de verdad (WebView2), con `--setup-preview`: datos de ejemplo y pasos
//! simulados, sin instalar nada. Necesita escritorio y WebView2:
//! `cargo test --release -p anotalo --test setup_window -- --ignored`

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, IsZoomed, PostMessageW,
    SC_MAXIMIZE, SendMessageW, WM_CLOSE, WM_SYSCOMMAND,
};
use windows::core::w;

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < end, "no llegó a tiempo: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// La ventana "Instalar Anotalo" de ese proceso.
fn setup_window(pid: u32) -> Option<HWND> {
    // SAFETY: busca una ventana por clase y título; lee de quién es.
    unsafe {
        let hwnd = FindWindowW(w!("AnotaloMainWindow"), w!("Instalar Anotalo")).ok()?;
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut owner));
        (owner == pid && IsWindowVisible(hwnd).as_bool()).then_some(hwnd)
    }
}

fn size_of_window(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    // SAFETY: coordenadas físicas (el hilo, consciente del DPI por monitor) de otra ventana.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        GetWindowRect(hwnd, &mut r).unwrap();
    }
    (r.right - r.left, r.bottom - r.top)
}

#[test]
#[ignore = "necesita WebView2 y una sesión de escritorio"]
fn setup_preview_window_is_fixed_and_closes() {
    let exe = env!("CARGO_BIN_EXE_anotalo");
    let data = std::env::temp_dir().join("Anotalo-setup-webview");
    let mut app = Kill(
        Command::new(exe)
            .args(["--setup-preview", "install"])
            .env("ANOTALO_PROFILE", "dev")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let hwnd = wait_for("la ventana del instalador", Duration::from_secs(30), || {
        setup_window(app.0.id())
    });
    // 460×440 a la escala del monitor
    // SAFETY: sin punteros.
    let dpi = unsafe { GetDpiForWindow(hwnd) } as i32;
    let want = ((460 * dpi + 48) / 96, (440 * dpi + 48) / 96);
    assert_eq!(size_of_window(hwnd), want);
    // ni maximizar
    // SAFETY: mensaje sin punteros; espera a que la ventana lo atienda.
    unsafe {
        SendMessageW(
            hwnd,
            WM_SYSCOMMAND,
            Some(WPARAM(SC_MAXIMIZE as usize)),
            Some(LPARAM(0)),
        );
    }
    // SAFETY: sin punteros.
    assert!(!unsafe { IsZoomed(hwnd) }.as_bool(), "no se maximiza");
    assert_eq!(size_of_window(hwnd), want);
    assert!(data.is_dir(), "WebView2 guarda sus datos en %TEMP%");

    // cerrar (la X) sale del proceso y borra los datos de WebView2
    // SAFETY: mensaje sin punteros a una ventana de otro proceso.
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
    let status = wait_for("que se cierre", Duration::from_secs(20), || {
        app.0.try_wait().unwrap()
    });
    assert!(status.success(), "salió con {status}");
    assert!(!data.exists(), "quedaron los datos de WebView2");
    let beside = Path::new(exe).with_extension("exe.WebView2");
    assert!(!beside.exists(), "WebView2 no deja carpeta junto al exe");
}

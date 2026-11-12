//! La app de la bandeja de punta a punta: una nota con el atajo y salir desde el menú.
//! Necesita escritorio, micrófono y el modelo:
//! `cargo test --release -p anotalo --test tray_app -- --ignored`

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, SendMessageW, WM_COMMAND, WM_ENDSESSION, WM_HOTKEY,
    WM_QUERYENDSESSION,
};
use windows::core::w;

/// La app no se queda abierta si el test falla.
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
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn post(hwnd: HWND, msg: u32, wp: usize) {
    // SAFETY: mensaje sin punteros a una ventana de otro proceso.
    unsafe { PostMessageW(Some(hwnd), msg, WPARAM(wp), LPARAM(0)) }.expect("PostMessageW");
}

fn saved_note(out: &Path) -> Option<PathBuf> {
    std::fs::read_dir(out)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.to_string_lossy().ends_with("_Nota.md")
                && std::fs::read_to_string(p).is_ok_and(|t| t.contains("- Duración:"))
        })
}

/// Arranca la app (perfil dev, carpetas temporales) y espera a que el modelo esté listo.
fn launch(tmp: &Path) -> (Kill, PathBuf, HWND) {
    let appdata = tmp.join("appdata");
    let out = tmp.join("out");
    let data = appdata.join("Anotalo-dev");
    std::fs::create_dir_all(&data).unwrap();
    // sin copiar al portapapeles: el test no pisa lo que haya copiado quien lo ejecuta
    let cfg = serde_json::json!({ "output_dir": out, "auto_mode": false, "copy_on_stop": false });
    std::fs::write(data.join("config.json"), cfg.to_string()).unwrap();

    let mut app = Kill(
        Command::new(env!("CARGO_BIN_EXE_anotalo"))
            .env("ANOTALO_PROFILE", "dev")
            .env("APPDATA", &appdata)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let log = data.join("app.log");
    wait_for("el modelo listo", Duration::from_secs(600), || {
        if let Ok(Some(st)) = app.0.try_wait() {
            panic!("la app terminó antes de tiempo ({st})");
        }
        std::fs::read_to_string(&log)
            .ok()
            .filter(|l| l.contains("Modelo listo"))
    });
    // SAFETY: busca una ventana por su clase.
    let hwnd = wait_for("la ventana de la app", Duration::from_secs(10), || {
        unsafe { FindWindowW(w!("AnotaloMessageWindow"), None) }.ok()
    });
    (app, out, hwnd)
}

#[test]
#[ignore]
fn tray_app_records_a_note_with_the_hotkey() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, out, hwnd) = launch(tmp.path());

    post(hwnd, WM_HOTKEY, 1); // empieza la nota
    std::thread::sleep(Duration::from_secs(3));
    post(hwnd, WM_HOTKEY, 1); // la para
    wait_for("la nota guardada", Duration::from_secs(120), || {
        saved_note(&out)
    });

    post(hwnd, WM_COMMAND, 5); // "Salir" del menú
    let status = wait_for("que la app se cierre", Duration::from_secs(10), || {
        app.0.try_wait().unwrap()
    });
    assert!(status.success(), "salió con {status}");
}

/// Apagar Windows grabando: al volver de `WM_ENDSESSION` (tras lo que Windows cierra el proceso),
/// la nota ya está guardada.
#[test]
#[ignore]
fn tray_app_saves_on_windows_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let (_app, out, hwnd) = launch(tmp.path());

    post(hwnd, WM_HOTKEY, 1);
    wait_for("la nota empezada", Duration::from_secs(10), || {
        std::fs::read_dir(&out).ok()?.flatten().next()
    });
    std::thread::sleep(Duration::from_secs(2));
    // SAFETY: mensajes sin punteros; espera a que la app los atienda.
    unsafe {
        assert_eq!(
            SendMessageW(hwnd, WM_QUERYENDSESSION, None, Some(LPARAM(0))),
            LRESULT(1)
        );
        SendMessageW(hwnd, WM_ENDSESSION, Some(WPARAM(1)), Some(LPARAM(0)));
    }
    assert!(
        saved_note(&out).is_some(),
        "la nota no se guardó antes de volver"
    );
}

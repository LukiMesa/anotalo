//! La app de la bandeja de punta a punta: una nota con el atajo y salir desde el menú.
//! Necesita escritorio, micrófono y el modelo:
//! `cargo test --release -p anotalo --test tray_app -- --ignored`

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Shutdown::ShutdownBlockReasonQuery;
use windows::Win32::UI::WindowsAndMessaging::{
    ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, FindWindowW, PostMessageW, SendMessageW, WM_COMMAND,
    WM_ENDSESSION, WM_HOTKEY, WM_QUERYENDSESSION,
};
use windows::core::w;

/// Tras "Anotalo cerrado", el proceso puede tardar unos segundos en terminar (en el equipo de
/// pruebas, con el modelo en la GPU, hasta ~10 s descargando las DLL).
const EXIT_WAIT: Duration = Duration::from_secs(30);

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

fn app_log(tmp: &Path) -> PathBuf {
    tmp.join("appdata").join("Anotalo-dev").join("app.log")
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
    let log = app_log(tmp);
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
    let status = wait_for("que la app se cierre", EXIT_WAIT, || {
        app.0.try_wait().unwrap()
    });
    assert!(status.success(), "salió con {status}");
}

/// ¿La ventana tiene un motivo para frenar el apagado ("Guardando la transcripción…")?
fn blocks_shutdown(hwnd: HWND) -> bool {
    let mut len = 0u32;
    // SAFETY: solo pide el tamaño del texto.
    unsafe { ShutdownBlockReasonQuery(hwnd, None, &mut len) }.is_ok()
}

/// Empieza una nota con el atajo y espera a que se esté grabando: el archivo aparece antes de que
/// el micrófono se abra, y abrirlo puede tardar segundos (mientras, aún no hay grabación).
fn start_note(hwnd: HWND, tmp: &Path) {
    post(hwnd, WM_HOTKEY, 1);
    wait_for("la nota grabando", Duration::from_secs(30), || {
        std::fs::read_to_string(app_log(tmp))
            .ok()
            .filter(|l| l.contains("Capturando"))
    });
    std::thread::sleep(Duration::from_secs(2));
}

/// Apaga Windows grabando con estas banderas de `WM_ENDSESSION`: al volver (tras lo que Windows
/// suele cerrar el proceso) la nota ya está guardada, con su cabecera completa. Y si Windows no lo
/// cierra (el gestor de reinicios de un instalador), la app sale sola: sin bandeja ni atajos no se
/// queda con la instancia única.
fn shutdown_while_recording(flags: u32) {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, out, hwnd) = launch(tmp.path());

    start_note(hwnd, tmp.path());
    // SAFETY: mensajes sin punteros; espera a que la app los atienda.
    unsafe {
        assert_eq!(
            SendMessageW(hwnd, WM_QUERYENDSESSION, None, Some(LPARAM(flags as isize))),
            LRESULT(1)
        );
        assert!(blocks_shutdown(hwnd), "sin \"Guardando la transcripción…\"");
        SendMessageW(
            hwnd,
            WM_ENDSESSION,
            Some(WPARAM(1)),
            Some(LPARAM(flags as isize)),
        );
    }
    assert!(
        saved_note(&out).is_some(),
        "la nota no se guardó antes de volver"
    );
    let status = wait_for("que la app salga sola", EXIT_WAIT, || {
        app.0.try_wait().unwrap()
    });
    assert!(status.success(), "salió con {status}");
}

#[test]
#[ignore]
fn tray_app_saves_on_windows_shutdown() {
    shutdown_while_recording(ENDSESSION_CLOSEAPP);
}

/// Un apagado forzado (`ENDSESSION_CRITICAL`): no espera a las últimas frases, pero el archivo se
/// cierra con su `- Duración:`.
#[test]
#[ignore]
fn tray_app_saves_on_forced_shutdown() {
    shutdown_while_recording(ENDSESSION_CRITICAL);
}

/// Un apagado que se cancela (otra app lo frenó) quita "Guardando la transcripción…" y la app sigue.
#[test]
#[ignore]
fn tray_app_cancelled_shutdown_clears_the_block_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, out, hwnd) = launch(tmp.path());

    start_note(hwnd, tmp.path());
    // SAFETY: mensajes sin punteros; espera a que la app los atienda.
    unsafe {
        SendMessageW(hwnd, WM_QUERYENDSESSION, None, Some(LPARAM(0)));
        assert!(blocks_shutdown(hwnd), "sin \"Guardando la transcripción…\"");
        SendMessageW(hwnd, WM_ENDSESSION, Some(WPARAM(0)), Some(LPARAM(0)));
    }
    assert!(
        !blocks_shutdown(hwnd),
        "el motivo sigue tras cancelarse el apagado"
    );
    assert!(app.0.try_wait().unwrap().is_none(), "la app sigue abierta");

    post(hwnd, WM_HOTKEY, 1); // para la nota
    wait_for("la nota guardada", Duration::from_secs(120), || {
        saved_note(&out)
    });
    post(hwnd, WM_COMMAND, 5); // "Salir"
    let status = wait_for("que la app se cierre", EXIT_WAIT, || {
        app.0.try_wait().unwrap()
    });
    assert!(status.success(), "salió con {status}");
}

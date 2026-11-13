//! La app de la bandeja de punta a punta: una nota con el atajo, la ventana principal y salir
//! desde el menú. Necesita escritorio, micrófono, WebView2 y el modelo:
//! `cargo test --release -p anotalo --test tray_app -- --ignored --test-threads=1`

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Shutdown::ShutdownBlockReasonQuery;
use windows::Win32::UI::WindowsAndMessaging::{
    ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, FindWindowW, IsWindowVisible, PostMessageW,
    SendMessageW, WM_APP, WM_CLOSE, WM_COMMAND, WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONUP,
    WM_QUERYENDSESSION,
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
    post_lp(hwnd, msg, wp, 0);
}

fn post_lp(hwnd: HWND, msg: u32, wp: usize, lp: isize) {
    // SAFETY: mensaje sin punteros a una ventana de otro proceso.
    unsafe { PostMessageW(Some(hwnd), msg, WPARAM(wp), LPARAM(lp)) }.expect("PostMessageW");
}

/// El clic izquierdo en el icono de la bandeja (`WM_APP_TRAY`).
fn tray_click(hwnd: HWND) {
    post_lp(hwnd, WM_APP + 1, 0, WM_LBUTTONUP as isize);
}

fn log_has(tmp: &Path, what: &str) -> Option<String> {
    std::fs::read_to_string(app_log(tmp))
        .ok()
        .filter(|l| l.contains(what))
}

fn main_window() -> Option<HWND> {
    // SAFETY: busca una ventana por su clase.
    unsafe { FindWindowW(w!("AnotaloMainWindow"), None) }.ok()
}

fn visible(hwnd: HWND) -> bool {
    // SAFETY: sin punteros.
    unsafe { IsWindowVisible(hwnd) }.as_bool()
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

/// Arranca la app (perfil dev, carpetas temporales, `--minimized`: la ventana no le quita el foco a
/// quien ejecuta los tests) y espera a que el modelo esté listo.
fn launch(tmp: &Path) -> (Kill, PathBuf, HWND) {
    launch_with(tmp, &[])
}

fn launch_with(tmp: &Path, env: &[(&str, &str)]) -> (Kill, PathBuf, HWND) {
    let appdata = tmp.join("appdata");
    let out = tmp.join("out");
    let data = appdata.join("Anotalo-dev");
    std::fs::create_dir_all(&data).unwrap();
    // sin copiar al portapapeles: el test no pisa lo que haya copiado quien lo ejecuta
    let cfg = serde_json::json!({ "output_dir": out, "auto_mode": false, "copy_on_stop": false });
    std::fs::write(data.join("config.json"), cfg.to_string()).unwrap();

    let mut app = Kill(
        Command::new(env!("CARGO_BIN_EXE_anotalo"))
            .arg("--minimized")
            .env("ANOTALO_PROFILE", "dev")
            .env("APPDATA", &appdata)
            .envs(env.iter().copied())
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

/// El instalador pide salir con el evento de salida (`AnotaloDevQuit` en el perfil dev): la nota en
/// curso se guarda y la app sale con 0. Solo la recibe la app de este test: si hubiera otra Anotalo
/// de desarrollo abierta, `launch` habría fallado (la segunda instancia sale enseguida).
#[test]
#[ignore]
fn quit_signal_saves_the_note_and_exits() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, out, hwnd) = launch(tmp.path());

    post(hwnd, WM_HOTKEY, 1); // empieza la nota
    std::thread::sleep(Duration::from_secs(3));
    assert!(app.0.try_wait().unwrap().is_none(), "la app sigue abierta");
    anotalo_winsys::instance::signal("AnotaloDevQuit");
    let status = wait_for("que la app se cierre", Duration::from_secs(60), || {
        app.0.try_wait().unwrap()
    });
    assert!(status.success(), "salió con {status}");
    assert!(saved_note(&out).is_some(), "la nota no se guardó");
    assert!(log_has(tmp.path(), "lo pide el instalador").is_some());
}

/// El instalador pide salir a una app que acaba de lanzar, mientras aún crea su ventana: la espera
/// de WebView2 se come el WM_QUIT. La app tiene que salir igual (con 0), sin el aviso de "Instala
/// WebView2" ni quedarse oculta con la instancia única.
#[test]
#[ignore]
fn quit_signal_while_the_window_opens() {
    use anotalo_winsys::instance;
    assert!(
        !instance::is_held("AnotaloDevMutex"),
        "hay otra Anotalo de desarrollo abierta: ciérrala antes"
    );
    let tmp = tempfile::tempdir().unwrap();
    let appdata = tmp.path().join("appdata");
    let data = appdata.join("Anotalo-dev");
    std::fs::create_dir_all(&data).unwrap();
    let cfg = serde_json::json!({ "output_dir": tmp.path().join("out"), "auto_mode": false, "copy_on_stop": false });
    std::fs::write(data.join("config.json"), cfg.to_string()).unwrap();
    let mut app = Kill(
        Command::new(env!("CARGO_BIN_EXE_anotalo"))
            .arg("--minimized")
            .env("ANOTALO_PROFILE", "dev")
            .env("APPDATA", &appdata)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for("la app con su instancia", Duration::from_secs(30), || {
        instance::is_held("AnotaloDevMutex").then_some(())
    });
    // desde ya, sin parar: la primera que llega es la del arranque
    let end = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(st) = app.0.try_wait().unwrap() {
            break st;
        }
        assert!(
            Instant::now() < end,
            "la app no salió: {:?}",
            std::fs::read_to_string(app_log(tmp.path()))
        );
        instance::signal("AnotaloDevQuit");
        std::thread::sleep(Duration::from_millis(5));
    };
    let log = std::fs::read_to_string(app_log(tmp.path())).unwrap();
    assert!(status.success(), "salió con {status}: {log}");
    assert!(log.contains("lo pide el instalador"), "{log}");
    assert!(!log.contains("No se pudo abrir la ventana"), "{log}");
    assert!(
        !log.contains("Anotalo iniciado"),
        "no arranca lo que ya se cierra: {log}"
    );
    assert!(
        log.contains("Se sale antes de terminar de arrancar"),
        "{log}"
    );
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

/// La ventana principal: empieza oculta con `--minimized`, el clic en el icono la muestra con la
/// página cargada, cerrarla solo la oculta (y guarda dónde estaba) y "Salir" cierra la app.
#[test]
#[ignore]
fn tray_app_window_shows_and_lists() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, _out, hwnd) = launch(tmp.path());
    wait_for("la ventana creada", Duration::from_secs(20), || {
        log_has(tmp.path(), "Ventana principal lista (oculta)")
    });
    let win = wait_for("la ventana principal", Duration::from_secs(10), main_window);
    assert!(!visible(win), "--minimized: empieza oculta");

    tray_click(hwnd);
    wait_for("la ventana a la vista", Duration::from_secs(10), || {
        visible(win).then_some(())
    });
    wait_for("la página cargada", Duration::from_secs(30), || {
        log_has(tmp.path(), "Ventana: ready")
    });

    post(win, WM_CLOSE, 0);
    wait_for("la ventana oculta", Duration::from_secs(10), || {
        (!visible(win)).then_some(())
    });
    assert!(
        app.0.try_wait().unwrap().is_none(),
        "cerrar no sale de la app"
    );
    let cfg = tmp
        .path()
        .join("appdata")
        .join("Anotalo-dev")
        .join("config.json");
    wait_for(
        "el sitio de la ventana guardado",
        Duration::from_secs(10),
        || {
            std::fs::read_to_string(&cfg)
                .ok()
                .filter(|c| c.contains("\"window_rect\": ["))
        },
    );

    post(hwnd, WM_COMMAND, 5); // "Salir"
    let status = wait_for("que la app se cierre", EXIT_WAIT, || {
        app.0.try_wait().unwrap()
    });
    assert!(status.success(), "salió con {status}");
}

/// Sin WebView2 (aquí, un fallo simulado) la app avisa con el enlace de descarga y sigue
/// grabando desde la bandeja.
#[test]
#[ignore]
fn test_webview_failure_keeps_tray_app() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, out, hwnd) = launch_with(tmp.path(), &[("ANOTALO_FAKE_WEBVIEW_FAILURE", "1")]);
    wait_for("el aviso de WebView2", Duration::from_secs(20), || {
        log_has(
            tmp.path(),
            "Aviso: No se pudo abrir la ventana — Instala WebView2: https://go.microsoft.com/fwlink/p/?LinkId=2124703",
        )
    });
    assert!(main_window().is_none(), "sin ventana principal");

    post(hwnd, WM_HOTKEY, 1); // empieza la nota
    std::thread::sleep(Duration::from_secs(3));
    post(hwnd, WM_HOTKEY, 1); // la para
    wait_for("la nota guardada", Duration::from_secs(120), || {
        saved_note(&out)
    });

    post(hwnd, WM_COMMAND, 5); // "Salir"
    let status = wait_for("que la app se cierre", EXIT_WAIT, || {
        app.0.try_wait().unwrap()
    });
    assert!(status.success(), "salió con {status}");
}

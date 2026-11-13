//! `RealSystem` con procesos y nombres propios de la prueba: una copia de `PING.EXE` en una
//! carpeta temporal y un mutex/evento `AnotaloTestSetup<pid>`. Nunca cierra ni pide cerrar la app
//! real ni Transcriptor.
use std::path::Path;
use std::time::{Duration, Instant};

use anotalo_setup::system::{RealSystem, System};
use anotalo_winsys::instance;

#[test]
fn test_real_system_with_its_own_processes() {
    let d = tempfile::tempdir().unwrap();
    let name = format!("anotalo-prueba-{}.exe", std::process::id());
    let exe = d.path().join(&name);
    std::fs::copy(r"C:\Windows\System32\PING.EXE", &exe).unwrap();
    let mutex = format!("AnotaloTestSetup{}", std::process::id());
    let sys = RealSystem::with_names(&mutex, &format!("{mutex}Quit"));

    assert!(sys.processes_in(d.path()).is_empty());
    sys.launch(&exe, &["-n", "30", "127.0.0.1"]).unwrap();
    let t = Instant::now();
    let mut pids = sys.processes_in(d.path());
    while pids.is_empty() && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
        pids = sys.processes_in(d.path());
    }
    assert_eq!(pids.len(), 1, "{pids:?}");
    assert_eq!(sys.processes_named(&name.to_uppercase()), pids);
    // la carpeta se compara sin distinguir mayúsculas ni la barra final
    let upper = d.path().to_string_lossy().to_uppercase() + "\\";
    assert_eq!(sys.processes_in(Path::new(&upper)), pids);
    sys.terminate(&pids);
    assert!(sys.wait_gone(&pids, Duration::from_secs(5)));
    assert!(sys.processes_in(d.path()).is_empty());

    // la app: su mutex y su evento de salida
    assert!(!sys.app_running());
    assert!(sys.wait_app_gone(Duration::from_millis(10)));
    let guard = instance::acquire(&mutex).unwrap();
    assert!(sys.app_running());
    let (tx, rx) = std::sync::mpsc::channel();
    instance::on_signal(
        &format!("{mutex}Quit"),
        Box::new(move || {
            let _ = tx.send(());
        }),
    );
    std::thread::sleep(Duration::from_millis(100));
    sys.ask_app_to_quit();
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
    assert!(!sys.wait_app_gone(Duration::from_millis(300)));
    drop(guard);
    assert!(sys.wait_app_gone(Duration::from_secs(2)));
}

#[test]
fn test_our_own_process_is_never_listed() {
    let sys = RealSystem::new();
    let me = std::env::current_exe().unwrap();
    assert!(
        !sys.processes_in(me.parent().unwrap())
            .contains(&std::process::id())
    );
    let name = me.file_name().unwrap().to_string_lossy().into_owned();
    assert!(!sys.processes_named(&name).contains(&std::process::id()));
}

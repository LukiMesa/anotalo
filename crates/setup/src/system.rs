//! Los otros procesos: la app instalada (su mutex y su evento de salida), los `Transcriptor.exe` y
//! lo que corre desde la carpeta de instalación. Detrás de un trait para probar sin procesos reales.

use std::path::Path;
use std::time::{Duration, Instant};

use anotalo_core::paths::{Profile, instance_names, quit_event};
use anotalo_winsys::{instance, process};

/// Cuánto se espera a que la app instalada se cierre sola (guarda la grabación en curso).
pub const APP_QUIT_WAIT: Duration = Duration::from_secs(90);
/// Cuánto se espera a que terminen los `Transcriptor.exe` tras cerrarlos.
pub const LEGACY_QUIT_WAIT: Duration = Duration::from_secs(10);

/// Lo que el instalador necesita de los otros procesos. Nunca devuelve el pid propio.
pub trait System {
    /// La app instalada está abierta (tiene su mutex).
    fn app_running(&self) -> bool;
    /// Le pide que se cierre (guarda la grabación en curso y quita su icono).
    fn ask_app_to_quit(&self);
    /// Espera a que suelte su mutex; false si no lo hizo a tiempo.
    fn wait_app_gone(&self, t: Duration) -> bool;
    /// Procesos cuyo exe está en `dir`.
    fn processes_in(&self, dir: &Path) -> Vec<u32>;
    /// Procesos con ese nombre de exe (sin distinguir mayúsculas).
    fn processes_named(&self, exe: &str) -> Vec<u32>;
    /// Los termina de golpe.
    fn terminate(&self, pids: &[u32]);
    fn wait_gone(&self, pids: &[u32], t: Duration) -> bool;
    /// Lanza `exe` suelto (sigue vivo cuando el instalador termina).
    fn launch(&self, exe: &Path, args: &[&str]) -> Result<(), String>;
}

/// Los procesos de verdad. La app es la instalada: mutex `AnotaloMutex`, evento `AnotaloQuit`.
pub struct RealSystem {
    mutex: String,
    quit: String,
}

impl RealSystem {
    pub fn new() -> RealSystem {
        let (mutex, _) = instance_names(Profile::Installed);
        RealSystem::with_names(mutex, quit_event(Profile::Installed))
    }

    /// Con otro mutex y otro evento (las pruebas).
    pub fn with_names(mutex: &str, quit_event: &str) -> RealSystem {
        RealSystem {
            mutex: mutex.into(),
            quit: quit_event.into(),
        }
    }

    fn others() -> impl Iterator<Item = process::Proc> {
        let me = std::process::id();
        process::list().into_iter().filter(move |p| p.pid != me)
    }
}

impl Default for RealSystem {
    fn default() -> Self {
        RealSystem::new()
    }
}

fn norm(p: &Path) -> String {
    p.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
}

impl System for RealSystem {
    fn app_running(&self) -> bool {
        instance::is_held(&self.mutex)
    }

    fn ask_app_to_quit(&self) {
        instance::signal(&self.quit);
    }

    fn wait_app_gone(&self, t: Duration) -> bool {
        let deadline = Instant::now() + t;
        loop {
            if !instance::is_held(&self.mutex) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn processes_in(&self, dir: &Path) -> Vec<u32> {
        let dir = norm(dir);
        RealSystem::others()
            .filter(|p| {
                p.path
                    .as_deref()
                    .and_then(Path::parent)
                    .is_some_and(|d| norm(d) == dir)
            })
            .map(|p| p.pid)
            .collect()
    }

    fn processes_named(&self, exe: &str) -> Vec<u32> {
        RealSystem::others()
            .filter(|p| p.name.to_lowercase() == exe.to_lowercase())
            .map(|p| p.pid)
            .collect()
    }

    fn terminate(&self, pids: &[u32]) {
        for &pid in pids {
            if let Err(e) = process::terminate(pid) {
                log::warn!("{e}");
            }
        }
    }

    fn wait_gone(&self, pids: &[u32], t: Duration) -> bool {
        process::wait_exit(pids, t)
    }

    fn launch(&self, exe: &Path, args: &[&str]) -> Result<(), String> {
        let dir = exe.parent().unwrap_or(Path::new(r"C:\"));
        process::spawn_detached(exe, args, dir).map(|_| ())
    }
}

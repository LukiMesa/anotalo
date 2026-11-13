//! Un PC de mentira para instalar: todas las carpetas bajo una carpeta temporal y el registro bajo
//! la clave de prueba `HKCU\Software\AnotaloTest\<pid>-<n>` (se borra al terminar). Nunca toca la
//! instalación real de Anotalo o Transcriptor, ni procesos de verdad (`FakeSystem`).
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anotalo_setup::install::{Options, Payload, install};
use anotalo_setup::layout::Layout;
use anotalo_setup::system::System;
use anotalo_winsys::registry::{self, RegValue};
use anotalo_winsys::shortcut::{self, Shortcut};
use sha2::{Digest, Sha256};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyW};
use windows::core::w;

/// `share_mode` de `OpenOptions`: lo que deja hacer a otros con el archivo abierto.
pub const FILE_SHARE_READ: u32 = 1;
pub const FILE_SHARE_DELETE: u32 = 4;

pub struct Env {
    pub dir: tempfile::TempDir,
    pub l: Layout,
    key: String,
}

/// COM iniciado en un hilo que vive lo que el proceso de pruebas. `shortcut::create`/`read` inician
/// y cierran COM en cada llamada; con varias pruebas a la vez, el último cierre del proceso
/// (que descarga lo de COM) se cruza con otro hilo que está usando un `ShellLink`, y el proceso
/// cae (0xC0000409, ~1 de cada 6 ejecuciones). La app y el instalador no lo sufren: crean los
/// accesos directos de uno en uno, o con COM ya iniciado en el hilo de la ventana.
fn hold_com() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // SAFETY: este hilo no termina nunca.
            let _ = unsafe {
                windows::Win32::System::Com::CoInitializeEx(
                    None,
                    windows::Win32::System::Com::COINIT_MULTITHREADED,
                )
            };
            let _ = tx.send(());
            loop {
                std::thread::park();
            }
        });
        let _ = rx.recv();
    });
}

impl Env {
    pub fn new() -> Env {
        hold_com();
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let key = format!(r"Software\AnotaloTest\{}-{n}", std::process::id());
        // La clave propia se crea ya: mientras exista, nadie puede borrar `AnotaloTest` (cada
        // prueba que termina intenta borrarla si está vacía). Si justo la está borrando otra
        // prueba (de este proceso o de otro), crear debajo falla un momento: se reintenta.
        let mut tries = 0;
        while let Err(e) = registry::set_values(&key, &[]) {
            tries += 1;
            assert!(tries < 50, "{e}");
            std::thread::sleep(Duration::from_millis(20));
        }
        let dir = tempfile::tempdir().unwrap();
        let l = Layout::for_test(dir.path(), &key);
        Env { dir, l, key }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// El instalador recién descargado: `<root>\dl\Anotalo-Setup-9.9.9.exe`.
    pub fn setup_exe(&self) -> PathBuf {
        let dl = self.root().join("dl");
        fs::create_dir_all(&dl).unwrap();
        let exe = dl.join("Anotalo-Setup-9.9.9.exe");
        fs::write(&exe, b"MZ fake setup").unwrap();
        exe
    }

    /// Un Transcriptor instalado con Inno: sus archivos (y uno del usuario), su clave, sus tres
    /// accesos directos, sus ajustes y la caché de CUDA. Devuelve su carpeta.
    pub fn inno(&self) -> PathBuf {
        let dir = self.l.legacy_default_dir();
        fs::create_dir_all(dir.join("_internal")).unwrap();
        for (name, bytes) in [
            ("Transcriptor.exe", &b"MZ transcriptor"[..]),
            ("unins000.exe", b"MZ unins"),
            ("unins000.dat", b"dat"),
            (r"_internal\x.pyd", b"MZ pyd"),
            ("mis notas.txt", b"del usuario"),
        ] {
            fs::write(dir.join(name), bytes).unwrap();
        }
        // Inno guarda la carpeta con la barra final
        let location = format!(r"{}\", dir.display());
        registry::set_values(
            &self.l.inno_key(),
            &[
                ("InstallLocation", RegValue::Str(&location)),
                ("DisplayName", RegValue::Str("Transcriptor")),
                ("DisplayVersion", RegValue::Str("0.6.4")),
            ],
        )
        .unwrap();
        let exe = dir.join("Transcriptor.exe");
        for (lnk, args) in [
            (self.l.programs.join("Transcriptor.lnk"), ""),
            (self.l.desktop.join("Transcriptor.lnk"), ""),
            (
                self.l.startup.join("Transcriptor de llamadas.lnk"),
                "--minimized",
            ),
        ] {
            shortcut::create(
                &lnk,
                &Shortcut {
                    target: exe.clone(),
                    args: args.into(),
                    workdir: dir.clone(),
                    description: "Transcriptor".into(),
                    aumid: None,
                },
            )
            .unwrap();
        }
        fs::create_dir_all(self.l.legacy_data_dir()).unwrap();
        fs::write(
            self.l.legacy_data_dir().join("config.json"),
            "{\n  \"hotkey\": \"<ctrl>+<alt>+t\",\n  \"autostart\": true\n}",
        )
        .unwrap();
        let cuda = self.l.legacy_local_dir().join("cuda");
        fs::create_dir_all(&cuda).unwrap();
        fs::write(cuda.join("cublas.dll"), b"MZ cublas").unwrap();
        dir
    }

    /// Ajustes, voces y grabaciones (también en la papelera) del usuario.
    pub fn user_data(&self) -> Vec<PathBuf> {
        let rec = self.root().join("Documents").join("Llamadas");
        let files = vec![
            self.l.data_dir().join("config.json"),
            self.l.data_dir().join("voces.json"),
            rec.join("a.md"),
            rec.join(".papelera").join("b.md"),
        ];
        for (f, text) in files.iter().zip(["{}", "{\"voces\": []}", "# a", "# b"]) {
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, text).unwrap();
        }
        files
    }

    /// Anotalo 9.9.9 instalado, con inicio automático y acceso directo en el escritorio.
    pub fn installed(&self) {
        let exe = self.setup_exe();
        install(
            &self.l,
            &payload(&exe),
            Options {
                autostart: true,
                desktop: true,
            },
            true,
            &FakeSystem::default(),
            &mut |_| {},
        )
        .unwrap();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = registry::delete_tree(&self.key);
        // SAFETY: nombre constante; solo borra la clave si no le quedan subclaves (otras pruebas).
        let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, w!(r"Software\AnotaloTest")) };
    }
}

pub fn payload(exe: &Path) -> Payload<'_> {
    Payload {
        exe,
        vcomp: b"MZ vcomp",
        version: "9.9.9",
    }
}

/// Otros procesos, de mentira: anota lo que se le pide.
#[derive(Default)]
pub struct FakeSystem {
    /// Anotalo está abierto (tiene su mutex).
    pub app_running: bool,
    /// Se cierra cuando se le pide.
    pub app_quits: bool,
    /// Procesos `Transcriptor.exe`.
    pub legacy_pids: Vec<u32>,
    /// Procesos que corren desde la carpeta de instalación.
    pub in_dir: Vec<u32>,
    pub log: Mutex<Vec<String>>,
}

impl FakeSystem {
    pub fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn record(&self, s: String) {
        self.log.lock().unwrap().push(s);
    }
}

impl System for FakeSystem {
    fn app_running(&self) -> bool {
        self.app_running
    }
    fn ask_app_to_quit(&self) {
        self.record("quit".into());
    }
    fn wait_app_gone(&self, _t: Duration) -> bool {
        self.app_quits
    }
    fn processes_in(&self, _dir: &Path) -> Vec<u32> {
        self.in_dir.clone()
    }
    fn processes_named(&self, exe: &str) -> Vec<u32> {
        if exe.eq_ignore_ascii_case("Transcriptor.exe") {
            self.legacy_pids.clone()
        } else {
            vec![]
        }
    }
    fn terminate(&self, pids: &[u32]) {
        self.record(format!("terminate {pids:?}"));
    }
    fn wait_gone(&self, _pids: &[u32], _t: Duration) -> bool {
        true
    }
    fn launch(&self, exe: &Path, args: &[&str]) -> Result<(), String> {
        self.record(format!("launch {} {}", exe.display(), args.join(" ")));
        Ok(())
    }
}

pub fn hashes(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|f| {
            Sha256::digest(fs::read(f).unwrap())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        })
        .collect()
}

/// Los `Anotalo.exe.old-*` de la carpeta de instalación.
pub fn old_files(l: &Layout) -> Vec<PathBuf> {
    fs::read_dir(l.install_dir())
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("Anotalo.exe.old-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

//! Desinstalar Anotalo (spec §5.8), en su sitio: se borra todo menos el propio exe, que lo borra al
//! salir un `cmd.exe` oculto de Windows (no hace falta copiarse a `%TEMP%`, que necesitaría el
//! mismo truco para borrarse). Las grabaciones (y su `.papelera`) y la caché de Hugging Face nunca
//! se tocan. Los ajustes y los modelos solo con la casilla "Borrar también los ajustes y los
//! modelos descargados" (`purge`); una desinstalación silenciosa lo conserva todo.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anotalo_winsys::registry;

use crate::install::close_app;
use crate::layout::{EXE_NAME, Layout, VCOMP};
use crate::legacy::{contains_any, recordings_dirs, remove_dir_all, remove_file};
use crate::system::{LEGACY_QUIT_WAIT, System};

/// `CREATE_NO_WINDOW` y `CREATE_BREAKAWAY_FROM_JOB` de `CreateProcess`.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const ERROR_ACCESS_DENIED: i32 = 5;

/// Cómo fue: lo que no se pudo borrar, y el exe que se borra al salir (el que desinstala).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    pub problems: Vec<String>,
    pub deferred: Option<PathBuf>,
}

/// Desinstala Anotalo de `l`. `self_exe`: el exe que está desinstalando (si es el instalado, se
/// deja para `schedule_self_delete`).
pub fn uninstall(l: &Layout, purge: bool, sys: &dyn System, self_exe: &Path) -> Removal {
    log::info!(
        "Desinstalando Anotalo de {} (borrar ajustes y modelos: {purge})",
        l.install_dir().display()
    );
    // Antes de borrar nada: las carpetas de grabaciones salen de los config.json, que se borran.
    let recordings = recordings_dirs(l);
    let mut problems = Vec::new();
    let mut note = |r: Result<(), String>| {
        if let Err(e) = r {
            log::warn!("{e}");
            problems.push(e);
        }
    };

    // 1. La app (guarda la grabación en curso) y lo que siga corriendo desde la carpeta
    //    (`Anotalo.exe mcp`). Nunca este proceso.
    close_app(l, sys);
    let others = sys.processes_in(&l.install_dir());
    if !others.is_empty() {
        log::info!("Termino lo que sigue abierto desde la carpeta: {others:?}");
        sys.terminate(&others);
        if !sys.wait_gone(&others, LEGACY_QUIT_WAIT) {
            log::warn!("No terminaron a tiempo: {others:?}");
        }
    }

    // 2. Accesos directos y claves.
    for lnk in [l.start_menu_lnk(), l.startup_lnk(), l.desktop_lnk()] {
        note(remove_file(&lnk));
    }
    note(registry::delete_tree(&l.uninstall_key()));
    note(registry::delete_tree(&l.aumid_key));

    // 3. El programa: vcomp140.dll, restos de actualizaciones y el exe (salvo el propio).
    let dir = l.install_dir();
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            if is_program_file(&e.file_name().to_string_lossy()) {
                note(remove_file(&e.path()));
            }
        }
    }
    let exe = l.exe();
    let mut deferred = None;
    if same_path(&exe, self_exe) {
        if exe.exists() {
            deferred = Some(exe);
        }
    } else {
        note(remove_file(&exe));
    }
    if deferred.is_none() && fs::remove_dir(&dir).is_ok() {
        log::info!("Carpeta borrada: {}", dir.display());
    }

    // 4. Las descargas de actualizaciones. 5. Con la casilla: ajustes y modelos de Anotalo y de la
    //    app de Python. Nunca una carpeta que tenga dentro las grabaciones.
    let mut trees = vec![l.temp.join("Anotalo-update")];
    if purge {
        trees.extend([
            l.data_dir(),
            l.local_dir(),
            l.legacy_data_dir(),
            l.legacy_local_dir(),
        ]);
    }
    for d in trees {
        if contains_any(&d, &recordings) {
            note(Err(format!(
                "No borro {}: contiene tus grabaciones",
                d.display()
            )));
        } else {
            note(remove_dir_all(&d));
        }
    }
    log::info!("Anotalo desinstalado");
    Removal { problems, deferred }
}

/// `vcomp140.dll`, y lo que dejan las actualizaciones (`Anotalo.exe.old-*`, `*.new`). El propio
/// `Anotalo.exe` va aparte.
fn is_program_file(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == VCOMP.to_ascii_lowercase()
        || [EXE_NAME, VCOMP].iter().any(|base| {
            let base = base.to_ascii_lowercase();
            name.starts_with(&format!("{base}.old-")) || name == format!("{base}.new")
        })
}

fn same_path(a: &Path, b: &Path) -> bool {
    let low = |p: &Path| p.to_string_lossy().to_lowercase();
    low(a) == low(b)
        || matches!(
            (fs::canonicalize(a), fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// La línea de `cmd.exe` que borra el exe y su carpeta (si queda vacía) cuando este proceso ya ha
/// terminado: espera ~2 s con `ping`, `del` y `rd`. None si alguna ruta tiene `%`, `!` o `"`, que
/// `cmd` interpretaría.
pub fn deferred_delete_args(exe: &Path, dir: &Path) -> Option<String> {
    let (exe, dir) = (exe.to_str()?, dir.to_str()?);
    if [exe, dir].iter().any(|s| s.contains(['%', '!', '"'])) {
        return None;
    }
    Some(format!(
        r#"/d /s /c "ping -n 3 127.0.0.1 >nul & del /f /q "{exe}" & rd "{dir}"""#
    ))
}

/// Lanza el `cmd.exe` de Windows, oculto, que borra `exe` y `dir` cuando este proceso termine. Hay
/// que llamarlo justo antes de salir: si el proceso sigue vivo pasados ~2 s, el exe no se borra.
/// Su carpeta de trabajo es System32: así `ping` es el de Windows y `cmd` no retiene la carpeta
/// que tiene que borrar.
pub fn schedule_self_delete(exe: &Path, dir: &Path) -> Result<(), String> {
    let args = deferred_delete_args(exe, dir).ok_or_else(|| {
        format!(
            "no se puede borrar {} al salir: la ruta tiene % o !",
            exe.display()
        )
    })?;
    let system32 = system32();
    let spawn = |flags: u32| {
        Command::new(system32.join("cmd.exe"))
            .raw_arg(&args)
            .current_dir(&system32)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    // Fuera del trabajo (job) de quien lanzó el desinstalador, si lo deja: si no, al cerrarse
    // este proceso podría llevarse el cmd por delante.
    let child = match spawn(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB) {
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => spawn(CREATE_NO_WINDOW),
        r => r,
    }
    .map_err(|e| format!("no se pudo programar el borrado de {} ({e})", exe.display()))?;
    log::info!(
        "{} se borrará al salir (cmd.exe, pid {})",
        exe.display(),
        child.id()
    );
    Ok(())
}

/// `%SystemRoot%\System32`.
fn system32() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
}

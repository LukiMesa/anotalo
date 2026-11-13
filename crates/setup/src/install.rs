//! Instalar o actualizar Anotalo (spec §5.8), y sustituir al Transcriptor de Inno.
//!
//! Orden: cerrar la app (y Transcriptor), copiar y comprobar Anotalo, crear los accesos directos y
//! la clave de desinstalación, y solo entonces quitar Transcriptor. Si algo falla antes de eso, se
//! deja todo como estaba (el exe anterior vuelve a su sitio) y se vuelve a abrir lo que estaba
//! abierto: una actualización silenciosa nunca deja al usuario sin app.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anotalo_winsys::registry::{self, RegValue};
use anotalo_winsys::shortcut::{self, Shortcut};
use sha2::{Digest, Sha256};

use crate::autostart;
use crate::layout::{AUMID, EXE_NAME, LEGACY_EXE, Layout, VCOMP};
use crate::legacy::{
    InnoInstall, find_inno, legacy_autostart, legacy_desktop, remove_cuda_cache, remove_file,
    remove_inno,
};
use crate::system::{APP_QUIT_WAIT, LEGACY_QUIT_WAIT, System};

/// Las casillas del instalador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// "Iniciar con Windows".
    pub autostart: bool,
    /// "Acceso directo en el escritorio".
    pub desktop: bool,
}

/// Lo que hay en el PC antes de instalar.
#[derive(Clone, Debug)]
pub struct Found {
    /// La versión de Anotalo instalada (DisplayVersion de su clave), si la hay.
    pub anotalo: Option<String>,
    pub inno: Option<InnoInstall>,
    pub app_running: bool,
    pub legacy_running: bool,
    /// Las casillas por defecto: con Anotalo instalado, como estén sus accesos directos; si se
    /// sustituye a Transcriptor, como los tenía él; en un PC nuevo, inicio con Windows sí y
    /// escritorio no.
    pub defaults: Options,
}

pub fn inspect(l: &Layout, sys: &dyn System) -> Found {
    let anotalo =
        registry::get_string(&l.uninstall_key(), "DisplayVersion").filter(|v| !v.trim().is_empty());
    let defaults = if anotalo.is_some() {
        Options {
            autostart: autostart::is_enabled(l),
            desktop: l.desktop_lnk().exists(),
        }
    } else {
        Options {
            autostart: legacy_autostart(l).unwrap_or(true),
            desktop: legacy_desktop(l),
        }
    };
    Found {
        anotalo,
        inno: find_inno(l),
        app_running: sys.app_running(),
        legacy_running: !sys.processes_named(LEGACY_EXE).is_empty(),
        defaults,
    }
}

/// Por dónde va la instalación (la ventana del instalador lo enseña).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Closing,
    Copying,
    Shortcuts,
    Removing,
    Launching,
    Done,
}

/// Lo que se instala: el exe (este mismo), la `vcomp140.dll` que lleva dentro y su versión.
pub struct Payload<'a> {
    pub exe: &'a Path,
    pub vcomp: &'a [u8],
    pub version: &'a str,
}

/// Instala `p` en `l`. `silent`: sin ventana; la app se abre con `--minimized`.
pub fn install(
    l: &Layout,
    p: &Payload,
    o: Options,
    silent: bool,
    sys: &dyn System,
    step: &mut dyn FnMut(Step),
) -> Result<(), String> {
    log::info!(
        "Instalando Anotalo {} en {} (silencioso: {silent}, inicio con Windows: {}, escritorio: {})",
        p.version,
        l.install_dir().display(),
        o.autostart,
        o.desktop
    );
    step(Step::Closing);
    let inno = find_inno(l);
    let was = close_running(l, sys);

    step(Step::Copying);
    let mut aside = Vec::new();
    if let Err(e) = copy_files(l, p, &mut aside) {
        return Err(give_back(l, sys, &was, inno.as_ref(), &aside, e));
    }
    step(Step::Shortcuts);
    if let Err(e) = shortcuts(l, p, o) {
        return Err(give_back(l, sys, &was, inno.as_ref(), &aside, e));
    }

    step(Step::Removing);
    if let Some(i) = &inno {
        log::info!("Quitando Transcriptor de {}", i.dir.display());
        let problems = remove_inno(l, i);
        if !problems.is_empty() {
            log::warn!("Transcriptor no se quitó del todo: {}", problems.join("; "));
        }
    }
    if let Err(e) = remove_cuda_cache(l) {
        log::warn!("{e}");
    }
    // El exe apartado ahora puede estar en uso (un `Anotalo.exe mcp`): lo borra la app al arrancar.
    let keep: Vec<PathBuf> = aside.iter().map(|(_, old)| old.clone()).collect();
    clean_leftovers_except(l, &keep);

    step(Step::Launching);
    let args: &[&str] = if silent { &["--minimized"] } else { &[] };
    sys.launch(&l.exe(), args)
        .map_err(|e| format!("Anotalo se instaló, pero no se pudo abrir: {e}"))?;
    log::info!("Anotalo {} instalado", p.version);
    step(Step::Done);
    Ok(())
}

/// Lo que estaba abierto antes de instalar.
struct Running {
    app: bool,
    legacy: bool,
}

/// Cierra Anotalo y Transcriptor (ver `close_app`). Los `Transcriptor.exe` se terminan sin más
/// (Python 0.6.x no tiene señal de salida; Inno hacía `taskkill /F`).
fn close_running(l: &Layout, sys: &dyn System) -> Running {
    let app = close_app(l, sys);
    let legacy = sys.processes_named(LEGACY_EXE);
    if !legacy.is_empty() {
        log::info!("Cierro Transcriptor ({legacy:?})");
        sys.terminate(&legacy);
        if !sys.wait_gone(&legacy, LEGACY_QUIT_WAIT) {
            log::warn!("Transcriptor no terminó a tiempo");
        }
    }
    Running {
        app,
        legacy: !legacy.is_empty(),
    }
}

/// Pide a Anotalo que se cierre (guarda la grabación en curso) y, solo si no lo hace a tiempo,
/// termina lo que corre desde su carpeta. Los `Anotalo.exe mcp` no tienen el mutex: si la app se
/// cierra bien, siguen en marcha. Devuelve si la app estaba abierta.
pub(crate) fn close_app(l: &Layout, sys: &dyn System) -> bool {
    let app = sys.app_running();
    if app {
        log::info!("Pido a Anotalo que se cierre");
        sys.ask_app_to_quit();
        if !sys.wait_app_gone(APP_QUIT_WAIT) {
            let pids = sys.processes_in(&l.install_dir());
            log::warn!("Anotalo no se cerró a tiempo; termino {pids:?}");
            if !pids.is_empty() {
                sys.terminate(&pids);
                sys.wait_gone(&pids, LEGACY_QUIT_WAIT);
            }
        }
    }
    app
}

/// Copia el exe y `vcomp140.dll` byte a byte (nunca `CopyFile`: el exe descargado lleva la marca de
/// la web, y la llevaría también el instalado) y comprueba su huella. Lo que había se aparta
/// renombrándolo (`<archivo>.old-<ms>`): un exe en uso se puede renombrar aunque no sobrescribir.
fn copy_files(l: &Layout, p: &Payload, aside: &mut Vec<(PathBuf, PathBuf)>) -> Result<(), String> {
    let bytes =
        fs::read(p.exe).map_err(|e| format!("no se pudo leer {} ({e})", p.exe.display()))?;
    let dir = l.install_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("no se pudo crear {} ({e})", dir.display()))?;
    let exe = l.exe();
    replace(&exe, &bytes, aside)?;
    let vcomp = dir.join(VCOMP);
    if fs::read(&vcomp).ok().as_deref() != Some(p.vcomp) {
        replace(&vcomp, p.vcomp, aside)?;
    }
    verify(&exe, &bytes)?;
    verify(&vcomp, p.vcomp)
}

/// Escribe `bytes` en `<target>.new`, aparta el que hubiera y pone el nuevo en su sitio.
fn replace(target: &Path, bytes: &[u8], aside: &mut Vec<(PathBuf, PathBuf)>) -> Result<(), String> {
    let new = suffixed(target, ".new");
    write_synced(&new, bytes)?;
    if fs::symlink_metadata(target).is_ok() {
        let old = suffixed(target, &format!(".old-{}", unix_ms()));
        if let Err(e) = fs::rename(target, &old) {
            let _ = fs::remove_file(&new);
            return Err(format!("no se pudo apartar {} ({e})", target.display()));
        }
        aside.push((target.to_path_buf(), old));
    }
    fs::rename(&new, target).map_err(|e| {
        let _ = fs::remove_file(&new);
        format!("no se pudo poner {} ({e})", target.display())
    })
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("no se pudo escribir {} ({e})", path.display());
    let mut f = File::create(path).map_err(fail)?;
    f.write_all(bytes).map_err(fail)?;
    f.sync_all().map_err(fail)
}

fn verify(path: &Path, expected: &[u8]) -> Result<(), String> {
    let got = fs::read(path).map_err(|e| format!("no se pudo leer {} ({e})", path.display()))?;
    if Sha256::digest(&got) != Sha256::digest(expected) {
        return Err(format!("{} no quedó bien copiado", path.display()));
    }
    Ok(())
}

/// Algo falló antes de tocar Transcriptor: el exe anterior vuelve a su sitio y se abre otra vez
/// lo que estaba abierto (Anotalo, o si no Transcriptor), minimizado. Devuelve el error.
fn give_back(
    l: &Layout,
    sys: &dyn System,
    was: &Running,
    inno: Option<&InnoInstall>,
    aside: &[(PathBuf, PathBuf)],
    e: String,
) -> String {
    log::error!("No se pudo instalar: {e}");
    for (target, old) in aside.iter().rev() {
        let _ = fs::remove_file(target);
        match fs::rename(old, target) {
            Ok(()) => log::info!("{} vuelve a su sitio", target.display()),
            Err(err) => log::error!("No se pudo devolver {} ({err})", target.display()),
        }
    }
    let previous = if was.app {
        Some(l.exe())
    } else if was.legacy {
        inno.map(|i| i.dir.join(LEGACY_EXE))
    } else {
        None
    };
    if let Some(exe) = previous {
        log::info!("Vuelvo a abrir {}", exe.display());
        if let Err(err) = sys.launch(&exe, &["--minimized"]) {
            log::error!("{err}");
        }
    }
    e
}

/// Accesos directos (menú Inicio con el AppUserModelID de los avisos, inicio con Windows y
/// escritorio según las casillas) y la entrada de "Aplicaciones instaladas".
fn shortcuts(l: &Layout, p: &Payload, o: Options) -> Result<(), String> {
    let exe = l.exe();
    let dir = l.install_dir();
    let link = |aumid: Option<&str>| Shortcut {
        target: exe.clone(),
        args: String::new(),
        workdir: dir.clone(),
        description: "Anotalo".into(),
        aumid: aumid.map(str::to_string),
    };
    shortcut::create(&l.start_menu_lnk(), &link(Some(AUMID)))?;
    if o.autostart {
        autostart::enable(l)?;
    } else {
        autostart::disable(l)?;
    }
    if o.desktop {
        shortcut::create(&l.desktop_lnk(), &link(None))?;
    } else {
        remove_file(&l.desktop_lnk())?;
    }

    let size: u64 = [exe.clone(), dir.join(VCOMP)]
        .iter()
        .filter_map(|f| fs::metadata(f).ok())
        .map(|m| m.len())
        .sum();
    let exe_s = exe.display().to_string();
    let icon = format!("{exe_s},0");
    let uninstall = format!("\"{exe_s}\" --uninstall");
    let quiet = format!("\"{exe_s}\" --uninstall /S");
    let location = dir.display().to_string();
    let date = chrono::Local::now().format("%Y%m%d").to_string();
    registry::set_values(
        &l.uninstall_key(),
        &[
            ("DisplayName", RegValue::Str("Anotalo")),
            ("DisplayVersion", RegValue::Str(p.version)),
            ("Publisher", RegValue::Str("LukiMesa")),
            ("DisplayIcon", RegValue::Str(&icon)),
            ("UninstallString", RegValue::Str(&uninstall)),
            ("QuietUninstallString", RegValue::Str(&quiet)),
            ("InstallLocation", RegValue::Str(&location)),
            ("InstallDate", RegValue::Str(&date)),
            ("NoModify", RegValue::Dword(1)),
            ("NoRepair", RegValue::Dword(1)),
            (
                "EstimatedSize",
                RegValue::Dword(size.div_ceil(1024).min(u32::MAX as u64) as u32),
            ),
            (
                "URLInfoAbout",
                RegValue::Str("https://github.com/LukiMesa/anotalo"),
            ),
        ],
    )?;
    registry::set_values(&l.aumid_key, &[("DisplayName", RegValue::Str("Anotalo"))])
}

/// Borra lo que dejaron instalaciones anteriores en la carpeta: `Anotalo.exe.old-*`,
/// `vcomp140.dll.old-*` y `*.new`. Lo que esté en uso se queda para la próxima vez.
pub fn clean_leftovers(l: &Layout) {
    clean_leftovers_except(l, &[]);
}

fn clean_leftovers_except(l: &Layout, keep: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(l.install_dir()) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let leftover = [EXE_NAME, VCOMP]
            .iter()
            .any(|base| name.starts_with(&format!("{base}.old-")) || name == format!("{base}.new"));
        if leftover && !keep.contains(&e.path()) {
            match fs::remove_file(e.path()) {
                Ok(()) => log::info!("Borrado {name}"),
                Err(err) => log::debug!("No se pudo borrar {name} ({err})"),
            }
        }
    }
}

fn suffixed(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

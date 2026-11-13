//! Accesos directos (`.lnk`) y carpetas conocidas del usuario (Inicio, menú Inicio, escritorio).
//! El del menú Inicio lleva el AppUserModelID de los avisos de Windows.

use std::path::{Path, PathBuf};

use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemAlloc, CoTaskMemFree, CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT,
    SHGetKnownFolderPath, ShellLink,
};
use windows::core::{BSTR, HSTRING, Interface, PWSTR};

/// Lo que guarda un acceso directo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub target: PathBuf,
    pub args: String,
    pub workdir: PathBuf,
    pub description: String,
    /// `System.AppUserModel.ID`: enlaza los avisos de Windows con la app.
    pub aumid: Option<String>,
}

/// Carpetas del usuario donde van los accesos directos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    /// Inicio automático (`shell:startup`).
    Startup,
    /// Programas del menú Inicio (`shell:programs`).
    Programs,
    Desktop,
}

/// COM iniciado en este hilo mientras viva (si ya lo estaba en otro modo, se usa tal cual).
struct Com(bool);

impl Com {
    fn init() -> Com {
        // SAFETY: se empareja con CoUninitialize en Drop, en el mismo hilo, solo si tuvo éxito.
        Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: ver Com::init.
            unsafe { CoUninitialize() };
        }
    }
}

/// Crea (o reemplaza) el acceso directo `lnk`, y su carpeta si no existe.
pub fn create(lnk: &Path, s: &Shortcut) -> Result<(), String> {
    if let Some(dir) = lnk.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("no se pudo crear {} ({e})", dir.display()))?;
    }
    let _com = Com::init();
    let fail = |what: &str, e: windows::core::Error| {
        format!("no se pudo crear {} ({what}: {e})", lnk.display())
    };
    // SAFETY: interfaces COM de este hilo; cadenas válidas durante cada llamada.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| fail("ShellLink", e))?;
        link.SetPath(&HSTRING::from(s.target.as_os_str()))
            .map_err(|e| fail("destino", e))?;
        link.SetArguments(&HSTRING::from(s.args.as_str()))
            .map_err(|e| fail("argumentos", e))?;
        link.SetWorkingDirectory(&HSTRING::from(s.workdir.as_os_str()))
            .map_err(|e| fail("carpeta de trabajo", e))?;
        link.SetDescription(&HSTRING::from(s.description.as_str()))
            .map_err(|e| fail("descripción", e))?;
        if let Some(id) = &s.aumid {
            let store: IPropertyStore = link.cast().map_err(|e| fail("propiedades", e))?;
            let value = lpwstr_variant(id).ok_or_else(|| format!("sin memoria para {id}"))?;
            store
                .SetValue(&PKEY_AppUserModel_ID, &value)
                .map_err(|e| fail("AppUserModelID", e))?;
            store.Commit().map_err(|e| fail("AppUserModelID", e))?;
        }
        let file: IPersistFile = link.cast().map_err(|e| fail("IPersistFile", e))?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true)
            .map_err(|e| fail("guardar", e))?;
    }
    Ok(())
}

/// Lee un acceso directo (sin resolverlo: el destino puede no existir).
pub fn read(lnk: &Path) -> Result<Shortcut, String> {
    let _com = Com::init();
    let fail = |what: &str, e: windows::core::Error| {
        format!("no se pudo leer {} ({what}: {e})", lnk.display())
    };
    // SAFETY: interfaces COM de este hilo; búferes del tamaño indicado.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| fail("ShellLink", e))?;
        let file: IPersistFile = link.cast().map_err(|e| fail("IPersistFile", e))?;
        file.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ)
            .map_err(|e| fail("abrir", e))?;
        let mut buf = vec![0u16; 32_768];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0)
            .map_err(|e| fail("destino", e))?;
        let target = PathBuf::from(wide(&buf));
        buf.fill(0);
        link.GetArguments(&mut buf)
            .map_err(|e| fail("argumentos", e))?;
        let args = wide(&buf);
        buf.fill(0);
        link.GetWorkingDirectory(&mut buf)
            .map_err(|e| fail("carpeta de trabajo", e))?;
        let workdir = PathBuf::from(wide(&buf));
        buf.fill(0);
        link.GetDescription(&mut buf)
            .map_err(|e| fail("descripción", e))?;
        let description = wide(&buf);
        let aumid = link
            .cast::<IPropertyStore>()
            .and_then(|store| store.GetValue(&PKEY_AppUserModel_ID))
            .ok()
            .filter(|v| !v.is_empty())
            .and_then(|v| BSTR::try_from(&v).ok())
            .map(|b| b.to_string())
            .filter(|s| !s.is_empty());
        Ok(Shortcut {
            target,
            args,
            workdir,
            description,
            aumid,
        })
    }
}

/// La ruta de una carpeta conocida del usuario (`SHGetKnownFolderPath`).
pub fn known_folder(f: Folder) -> Result<PathBuf, String> {
    let id = match f {
        Folder::Startup => FOLDERID_Startup,
        Folder::Programs => FOLDERID_Programs,
        Folder::Desktop => FOLDERID_Desktop,
    };
    // SAFETY: la cadena devuelta es de COM y se libera aquí.
    unsafe {
        let p = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None)
            .map_err(|e| format!("no se encontró la carpeta {f:?} ({e})"))?;
        let path = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        path.map(PathBuf::from)
            .map_err(|e| format!("ruta no válida para {f:?} ({e})"))
    }
}

fn wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Un PROPVARIANT `VT_LPWSTR` (lo que hace `InitPropVariantFromString`), con la cadena en memoria
/// de COM: su Drop la libera (PropVariantClear).
fn lpwstr_variant(s: &str) -> Option<PROPVARIANT> {
    let units: Vec<u16> = s.encode_utf16().chain([0]).collect();
    // SAFETY: memoria de COM del tamaño justo, rellenada antes de usarla.
    unsafe {
        let mem = CoTaskMemAlloc(units.len() * 2) as *mut u16;
        if mem.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), mem, units.len());
        Some(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_LPWSTR,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        pwszVal: PWSTR(mem),
                    },
                }),
            },
        })
    }
}

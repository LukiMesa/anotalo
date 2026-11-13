//! Dónde va cada cosa al instalar: carpetas del usuario y claves del registro (de `HKCU`).

use std::path::{Path, PathBuf};

use anotalo_core::paths::{self, Profile};
use anotalo_winsys::shortcut::{self, Folder};

pub const EXE_NAME: &str = "Anotalo.exe";
pub const VCOMP: &str = "vcomp140.dll";
pub const LNK: &str = "Anotalo.lnk";
pub const UNINSTALL_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
pub const UNINSTALL_NAME: &str = "Anotalo";
/// La clave de desinstalación del Transcriptor instalado con Inno (`AppId` de `installer.iss`).
pub const INNO_KEY_NAME: &str = "{6B7C2D51-3A8E-4F5B-9C1D-2E4F6A8B0C13}_is1";
/// El nombre de la app en los avisos de Windows.
pub const AUMID_KEY: &str = r"Software\Classes\AppUserModelId\Anotalo";
pub const AUMID: &str = "Anotalo";
pub const LEGACY_EXE: &str = "Transcriptor.exe";
pub const LEGACY_LNK: &str = "Transcriptor.lnk";
pub const LEGACY_STARTUP_LNK: &str = "Transcriptor de llamadas.lnk";

/// Las carpetas y claves que toca el instalador. `reg_base` es la clave bajo la que están la de
/// Anotalo y la de Inno (la de desinstalación de Windows, o una de prueba).
#[derive(Clone, Debug)]
pub struct Layout {
    /// `%APPDATA%`
    pub appdata: PathBuf,
    /// `%LOCALAPPDATA%`
    pub local_appdata: PathBuf,
    /// `%TEMP%`
    pub temp: PathBuf,
    /// Programas del menú Inicio.
    pub programs: PathBuf,
    /// Inicio automático.
    pub startup: PathBuf,
    pub desktop: PathBuf,
    pub reg_base: String,
    pub aumid_key: String,
}

impl Layout {
    /// Las del usuario que ejecuta esto.
    pub fn current() -> Result<Layout, String> {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| format!("falta la variable de entorno {name}"))
        };
        Ok(Layout {
            appdata: var("APPDATA")?,
            local_appdata: var("LOCALAPPDATA")?,
            temp: std::env::temp_dir(),
            programs: shortcut::known_folder(Folder::Programs)?,
            startup: shortcut::known_folder(Folder::Startup)?,
            desktop: shortcut::known_folder(Folder::Desktop)?,
            reg_base: UNINSTALL_ROOT.into(),
            aumid_key: AUMID_KEY.into(),
        })
    }

    /// Todo bajo `root`, y las claves bajo `reg_base` (una de prueba).
    pub fn for_test(root: &Path, reg_base: &str) -> Layout {
        let appdata = root.join("AppData").join("Roaming");
        let programs = appdata
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        Layout {
            local_appdata: root.join("AppData").join("Local"),
            temp: root.join("Temp"),
            startup: programs.join("Startup"),
            programs,
            desktop: root.join("Desktop"),
            appdata,
            reg_base: reg_base.into(),
            aumid_key: format!(r"{reg_base}\AUMID"),
        }
    }

    /// `%LOCALAPPDATA%\Programs\Anotalo`
    pub fn install_dir(&self) -> PathBuf {
        paths::install_dir_in(&self.local_appdata)
    }

    pub fn exe(&self) -> PathBuf {
        self.install_dir().join(EXE_NAME)
    }

    pub fn uninstall_key(&self) -> String {
        format!(r"{}\{UNINSTALL_NAME}", self.reg_base)
    }

    pub fn inno_key(&self) -> String {
        format!(r"{}\{INNO_KEY_NAME}", self.reg_base)
    }

    pub fn start_menu_lnk(&self) -> PathBuf {
        self.programs.join(LNK)
    }

    pub fn desktop_lnk(&self) -> PathBuf {
        self.desktop.join(LNK)
    }

    pub fn startup_lnk(&self) -> PathBuf {
        self.startup.join(LNK)
    }

    /// Donde Inno instalaba Transcriptor si no se elegía otra carpeta.
    pub fn legacy_default_dir(&self) -> PathBuf {
        self.local_appdata.join("Programs").join("Transcriptor")
    }

    /// `%APPDATA%\Anotalo`: ajustes, voces, logs.
    pub fn data_dir(&self) -> PathBuf {
        paths::data_dir_in(&self.appdata, Profile::Installed)
    }

    /// `%LOCALAPPDATA%\Anotalo`: modelos.
    pub fn local_dir(&self) -> PathBuf {
        self.local_appdata.join("Anotalo")
    }

    /// `%APPDATA%\CallTranscriber`: los ajustes de la app de Python.
    pub fn legacy_data_dir(&self) -> PathBuf {
        paths::legacy_data_dir_in(&self.appdata, Profile::Installed)
    }

    /// `%LOCALAPPDATA%\Transcriptor`: la caché de CUDA de la app de Python.
    pub fn legacy_local_dir(&self) -> PathBuf {
        self.local_appdata.join("Transcriptor")
    }

    /// `%APPDATA%\Anotalo\setup.log`
    pub fn setup_log(&self) -> PathBuf {
        self.data_dir().join("setup.log")
    }
}

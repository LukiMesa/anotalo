//! Nuestro cargador de WebView2 (spec §5.1: el exe no lleva `WebView2LoaderStatic.lib`, el
//! cargador precompilado de Microsoft que enlazaba `webview2-com-sys`).
//!
//! Hace lo mismo que el de Microsoft con el runtime "Evergreen" instalado: busca en el registro la
//! carpeta de la versión, carga de ahí `EBWebView\<arq>\EmbeddedBrowserWebView.dll` (la parte del
//! runtime que crea el entorno) y llama a su `CreateWebViewEnvironmentWithOptionsInternal`. También
//! vale una carpeta de versión fija (`browserExecutableFolder` o la variable
//! `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER`). No busca los canales de prueba de Edge (Beta, Dev,
//! Canary) ni mira las directivas de empresa (`Software\Policies\Microsoft\Edge\WebView2`).
//!
//! `webview2-com-sys` (la copia de `third_party/`) declara las cinco funciones del cargador sin
//! enlazarlas con nada: son las `extern "system"` de abajo, con `#[unsafe(no_mangle)]`.

use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA,
    ERROR_PROC_NOT_FOUND, ERROR_SUCCESS, FreeLibrary, S_OK,
};
use windows::Win32::System::Com::CoTaskMemAlloc;
use windows::Win32::System::LibraryLoader::{
    FindResourceW, GetProcAddress, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW, LoadResource,
    LockResource, SizeofResource,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RegGetValueW,
};
use windows::Win32::UI::WindowsAndMessaging::RT_VERSION;
use windows::core::{HRESULT, HSTRING, PCWSTR, PWSTR, s};

/// Lo que ve el usuario si no hay WebView2.
pub const NOT_FOUND: &str =
    "No se encontró WebView2. Instálalo desde https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// El runtime de WebView2 en EdgeUpdate (el canal estable, "Evergreen").
const CLIENTS: &str =
    r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
const CLIENT_STATE: &str =
    r"SOFTWARE\Microsoft\EdgeUpdate\ClientState\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
/// La versión mínima del runtime (la misma que el cargador en Go de wails); las anteriores no se
/// usan.
const MIN_VERSION: Version = Version([86, 0, 616, 0]);
const ENV_BROWSER_FOLDER: &str = "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER";
const ENV_USER_DATA_FOLDER: &str = "WEBVIEW2_USER_DATA_FOLDER";
/// La carpeta de la arquitectura dentro de `EBWebView`.
const ARCH: &str = if cfg!(target_arch = "x86_64") {
    "x64"
} else if cfg!(target_arch = "aarch64") {
    "arm64"
} else {
    "x86"
};

/// Una versión del runtime ("157.0.4341.46").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub [u32; 4]);

impl Version {
    /// De 1 a 4 números separados por puntos (los que faltan valen 0). Lo que va tras un espacio
    /// es el canal ("78.0.276.0 dev") y no cuenta.
    pub fn parse(s: &str) -> Option<Version> {
        let mut out = [0; 4];
        let mut parts = s.split_whitespace().next()?.split('.');
        for (slot, part) in out.iter_mut().zip(parts.by_ref()) {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *slot = part.parse().ok()?;
        }
        parts.next().is_none().then_some(Version(out))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "{a}.{b}.{c}.{d}")
    }
}

/// Dónde está instalado el runtime: para todo el equipo (HKLM) o para el usuario (HKCU).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    Machine,
    User,
}

/// Lo que dice el registro de una instalación del runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    pub hive: Hive,
    /// `ClientState\{…}`, valor `EBWebView`: la carpeta de la versión (la que mira el cargador
    /// de Microsoft).
    pub ebwebview: Option<String>,
    /// `Clients\{…}`, valor `pv`: la versión.
    pub pv: Option<String>,
    /// `Clients\{…}`, valor `location`: la carpeta con una subcarpeta por versión.
    pub location: Option<String>,
}

/// Qué runtime es.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeKind {
    /// El instalado (Evergreen), de esta versión.
    Installed(Version),
    /// El de una carpeta de versión fija.
    Fixed,
}

/// El runtime elegido.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtime {
    /// `EmbeddedBrowserWebView.dll`, con su ruta completa.
    pub dll: PathBuf,
    pub kind: RuntimeKind,
}

impl Runtime {
    /// Su versión; la de una carpeta fija, la de su DLL (`None` si no se puede leer).
    pub fn version(&self) -> Option<Version> {
        match self.kind {
            RuntimeKind::Installed(v) => Some(v),
            RuntimeKind::Fixed => dll_version(&self.dll),
        }
    }
}

/// `<carpeta>\EBWebView\<arq>\EmbeddedBrowserWebView.dll`.
pub fn client_dll(folder: &Path) -> PathBuf {
    folder
        .join("EBWebView")
        .join(ARCH)
        .join("EmbeddedBrowserWebView.dll")
}

/// Las carpetas de versión que da una instalación, con su versión: la de `EBWebView` y la de
/// `location\pv` (suelen ser la misma).
fn version_folders(install: &Install) -> impl Iterator<Item = (Version, PathBuf)> {
    let state = install.ebwebview.as_deref().and_then(|folder| {
        let folder = PathBuf::from(folder);
        let version = Version::parse(folder.file_name()?.to_str()?)?;
        Some((version, folder))
    });
    let clients = match (install.pv.as_deref(), install.location.as_deref()) {
        (Some(pv), Some(location)) if !location.is_empty() => {
            Version::parse(pv).map(|v| (v, Path::new(location).join(pv.trim())))
        }
        _ => None,
    };
    state.into_iter().chain(clients)
}

/// La instalación válida con la versión más alta (si empatan, la de HKLM). Válida: versión que
/// se entiende y no menor que la mínima, carpeta absoluta y la DLL en su sitio (`exists`).
pub fn pick_installed(installs: &[Install], exists: impl Fn(&Path) -> bool) -> Option<Runtime> {
    installs
        .iter()
        .flat_map(|i| version_folders(i).map(move |(v, folder)| (v, i.hive, folder)))
        .filter(|(v, _, folder)| *v >= MIN_VERSION && folder.is_absolute())
        .map(|(v, hive, folder)| (v, hive, client_dll(&folder)))
        .filter(|(_, _, dll)| exists(dll))
        .max_by_key(|(v, hive, _)| (*v, *hive == Hive::Machine))
        .map(|(v, _, dll)| Runtime {
            dll,
            kind: RuntimeKind::Installed(v),
        })
}

/// `Some` si no está vacía.
fn non_empty(s: Option<&OsStr>) -> Option<&Path> {
    s.filter(|s| !s.is_empty()).map(Path::new)
}

/// La carpeta de versión fija que se pide: `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` (`env`) si no
/// está vacía, si no `param`. Si es relativa, respecto a la carpeta del exe.
pub fn fixed_folder(env: Option<&OsStr>, param: Option<&Path>, exe_dir: &Path) -> Option<PathBuf> {
    let folder = non_empty(env).or(non_empty(param.map(Path::as_os_str)))?;
    Some(exe_dir.join(folder)) // si `folder` es absoluta, `join` la deja tal cual
}

/// El runtime a usar: el de la carpeta fija si se pide (y solo ese), si no el instalado.
pub fn choose(
    fixed: Option<&Path>,
    installs: &[Install],
    exists: impl Fn(&Path) -> bool,
) -> Result<Runtime, String> {
    let Some(folder) = fixed else {
        return pick_installed(installs, exists).ok_or_else(|| NOT_FOUND.to_string());
    };
    let dll = client_dll(folder);
    if exists(&dll) {
        Ok(Runtime {
            dll,
            kind: RuntimeKind::Fixed,
        })
    } else {
        Err(format!("No se encontró WebView2 en {}", folder.display()))
    }
}

/// La carpeta de datos que se pide: `WEBVIEW2_USER_DATA_FOLDER` (`env`) si no está vacía, si no
/// `param`. `None`: la de por defecto (`default_user_data_folder`).
pub fn user_data_folder(env: Option<&OsStr>, param: Option<&Path>) -> Option<PathBuf> {
    non_empty(env)
        .or(non_empty(param.map(Path::as_os_str)))
        .map(Path::to_path_buf)
}

/// La de por defecto del cargador de Microsoft: `<exe>.WebView2` junto al exe.
pub fn default_user_data_folder(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".WebView2");
    exe.with_file_name(name)
}

/// La versión de producto de un recurso `VS_VERSIONINFO`: la de su `VS_FIXEDFILEINFO`, que va
/// alineado a 4 bytes tras la clave y empieza por la firma 0xFEEF04BD.
fn fixed_file_version(resource: &[u8]) -> Option<Version> {
    let word = |at: usize| {
        let bytes = resource.get(at..at + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    };
    let start = (0..resource.len())
        .step_by(4)
        .find(|&at| word(at) == Some(0xFEEF_04BD))?;
    let (ms, ls) = (word(start + 16)?, word(start + 20)?);
    Some(Version([ms >> 16, ms & 0xFFFF, ls >> 16, ls & 0xFFFF]))
}

/// La versión de producto de una DLL (leída como datos: no se ejecuta nada de ella).
fn dll_version(dll: &Path) -> Option<Version> {
    let path = HSTRING::from(dll);
    let flags = LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE;
    // SAFETY: la DLL se abre solo como recurso y se suelta al final; el recurso vive mientras
    // esté abierta y se lee dentro de su tamaño.
    unsafe {
        let module = LoadLibraryExW(&path, None, flags).ok()?;
        let read = || {
            let id = PCWSTR(std::ptr::without_provenance(1)); // VS_VERSION_INFO
            let res = FindResourceW(Some(module), id, RT_VERSION);
            if res.is_invalid() {
                return None;
            }
            let size = SizeofResource(Some(module), res) as usize;
            let data = LockResource(LoadResource(Some(module), res).ok()?) as *const u8;
            (!data.is_null())
                .then(|| fixed_file_version(std::slice::from_raw_parts(data, size)))
                .flatten()
        };
        let version = read();
        let _ = FreeLibrary(module);
        version
    }
}

/// Las instalaciones que dice el registro, en HKLM y en HKCU.
fn read_installs() -> Vec<Install> {
    [
        (HKEY_LOCAL_MACHINE, Hive::Machine),
        (HKEY_CURRENT_USER, Hive::User),
    ]
    .into_iter()
    .map(|(root, hive)| Install {
        hive,
        ebwebview: reg_string(root, CLIENT_STATE, "EBWebView"),
        pv: reg_string(root, CLIENTS, "pv"),
        location: reg_string(root, CLIENTS, "location"),
    })
    .collect()
}

/// Un valor de texto del registro. EdgeUpdate es de 32 bits: en un Windows de 64 su clave de HKLM
/// está bajo WOW6432Node (en HKCU no cambia nada).
fn reg_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6432KEY;
    let mut buf = vec![0u16; 260];
    for _ in 0..2 {
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: búfer del tamaño indicado; clave y valor terminados en NUL.
        let rc = unsafe {
            RegGetValueW(
                root,
                &key,
                &value,
                flags,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if rc == ERROR_SUCCESS {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            return Some(String::from_utf16_lossy(&buf[..len]));
        }
        if rc != ERROR_MORE_DATA {
            return None;
        }
        buf.resize((size as usize).div_ceil(2), 0);
    }
    None
}

/// El runtime de este PC (la variable `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` o el instalado), o lo
/// que hay que decirle al usuario.
pub fn find_runtime() -> Result<Runtime, String> {
    find_runtime_for(None)
}

/// Como `find_runtime`, con el `browserExecutableFolder` que se pasó al cargador.
fn find_runtime_for(param: Option<&Path>) -> Result<Runtime, String> {
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_dir = exe.parent().unwrap_or(Path::new(""));
    let env = std::env::var_os(ENV_BROWSER_FOLDER);
    match fixed_folder(env.as_deref(), param, exe_dir) {
        Some(folder) => choose(Some(&folder), &[], Path::is_file),
        None => choose(None, &read_installs(), Path::is_file),
    }
}

/// La cadena de un parámetro del cargador (nula o vacía: ninguna).
///
/// # Safety
/// `p`, nula o terminada en NUL.
unsafe fn param(p: PCWSTR) -> Option<OsString> {
    if p.is_null() {
        return None;
    }
    // SAFETY: terminada en NUL (contrato del cargador).
    let wide = unsafe { p.as_wide() };
    (!wide.is_empty()).then(|| OsString::from_wide(wide))
}

fn not_found() -> HRESULT {
    ERROR_FILE_NOT_FOUND.to_hresult()
}

/// `CreateWebViewEnvironmentWithOptionsInternal(1, tipo, datos, opciones, aviso)` de
/// `EmbeddedBrowserWebView.dll`. Sin documentar: es lo que llama el cargador de Microsoft (su
/// `CreateWebViewEnvironmentWithClientDll(dll, bool, WebView2RunTimeType, datos, opciones, aviso)`)
/// y los que lo imitan, como el de wails. El primero es un sí (se pasa entero: los 32 bits quedan
/// definidos). El tipo: 0, el runtime instalado; 1, una carpeta fija.
type CreateInternal =
    unsafe extern "system" fn(i32, i32, PCWSTR, *mut c_void, *mut c_void) -> HRESULT;

/// Crea el entorno; el aviso llega después, por la cola de mensajes del hilo.
///
/// # Safety
/// `options` y `handler`: los del contrato de `CreateCoreWebView2EnvironmentWithOptions`.
unsafe fn create_environment(
    browser_folder: Option<&Path>,
    user_data: Option<&Path>,
    options: *mut c_void,
    handler: *mut c_void,
) -> Result<(), (HRESULT, String)> {
    let runtime = find_runtime_for(browser_folder).map_err(|e| (not_found(), e))?;
    let env = std::env::var_os(ENV_USER_DATA_FOLDER);
    let data = match user_data_folder(env.as_deref(), user_data) {
        Some(data) => data,
        None => default_user_data_folder(
            &std::env::current_exe().map_err(|e| (E_FAIL, format!("¿dónde está el exe? {e}")))?,
        ),
    };
    let dll = HSTRING::from(runtime.dll.as_path());
    // SAFETY: ruta completa; sus dependencias, solo de su carpeta y de System32 (nunca de la del
    // exe, la actual ni el PATH). Se queda cargada: el entorno y lo que cree viven en ella.
    let module = unsafe {
        LoadLibraryExW(
            &dll,
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .map_err(|e| {
        (
            e.code(),
            format!("no se pudo cargar {}: {e}", runtime.dll.display()),
        )
    })?;
    // SAFETY: módulo cargado; nombre terminado en NUL.
    let create =
        unsafe { GetProcAddress(module, s!("CreateWebViewEnvironmentWithOptionsInternal")) }
            .ok_or_else(|| {
                let msg = format!(
                    "{} no tiene CreateWebViewEnvironmentWithOptionsInternal",
                    runtime.dll.display()
                );
                (ERROR_PROC_NOT_FOUND.to_hresult(), msg)
            })?;
    // SAFETY: la firma de esa función (ver `CreateInternal`).
    let create: CreateInternal = unsafe { std::mem::transmute(create) };
    let kind = match runtime.kind {
        RuntimeKind::Installed(_) => 0,
        RuntimeKind::Fixed => 1,
    };
    let data = HSTRING::from(data.as_path());
    // SAFETY: cadena viva durante la llamada; `options` y `handler`, los de quien llama.
    let hr = unsafe { create(1, kind, PCWSTR(data.as_ptr()), options, handler) };
    hr.ok()
        .map_err(|e| (hr, format!("{}: {e}", runtime.dll.display())))
}

/// `CreateCoreWebView2EnvironmentWithOptions` del cargador: crea el entorno de WebView2 y avisa a
/// `handler` cuando esté (por la cola de mensajes de este hilo, que debe ser STA).
///
/// # Safety
/// `browser_folder` y `user_data_folder`, nulas o terminadas en NUL; `options`, nulo o un
/// `ICoreWebView2EnvironmentOptions`; `handler`, un
/// `ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler`.
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn CreateCoreWebView2EnvironmentWithOptions(
    browser_folder: PCWSTR,
    user_data_folder: PCWSTR,
    options: *mut c_void,
    handler: *mut c_void,
) -> HRESULT {
    if handler.is_null() {
        return E_POINTER;
    }
    // SAFETY: el contrato de esta función.
    let result = unsafe {
        let (folder, data) = (param(browser_folder), param(user_data_folder));
        create_environment(
            folder.as_deref().map(Path::new),
            data.as_deref().map(Path::new),
            options,
            handler,
        )
    };
    match result {
        Ok(()) => S_OK,
        Err((hr, msg)) => {
            log::error!("WebView2: {msg}");
            hr
        }
    }
}

/// `CreateCoreWebView2Environment` del cargador: lo mismo sin opciones.
///
/// # Safety
/// Ver `CreateCoreWebView2EnvironmentWithOptions`.
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn CreateCoreWebView2Environment(handler: *mut c_void) -> HRESULT {
    // SAFETY: el mismo contrato.
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null_mut(),
            handler,
        )
    }
}

/// `CompareBrowserVersions` del cargador: -1, 0 o 1 en `result`.
///
/// # Safety
/// `version1` y `version2`, nulas o terminadas en NUL; `result`, un `i32` escribible.
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn CompareBrowserVersions(
    version1: PCWSTR,
    version2: PCWSTR,
    result: *mut i32,
) -> HRESULT {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: el contrato de esta función.
    let parse = |p| unsafe { param(p) }.and_then(|s| Version::parse(&s.to_string_lossy()));
    let (Some(a), Some(b)) = (parse(version1), parse(version2)) else {
        return E_INVALIDARG;
    };
    // SAFETY: `result` escribible.
    unsafe { *result = a.cmp(&b) as i32 };
    S_OK
}

/// `GetAvailableCoreWebView2BrowserVersionString` del cargador.
///
/// # Safety
/// Ver `GetAvailableCoreWebView2BrowserVersionStringWithOptions`.
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn GetAvailableCoreWebView2BrowserVersionString(
    browser_folder: PCWSTR,
    version_info: *mut PWSTR,
) -> HRESULT {
    // SAFETY: el mismo contrato.
    unsafe {
        GetAvailableCoreWebView2BrowserVersionStringWithOptions(
            browser_folder,
            std::ptr::null_mut(),
            version_info,
        )
    }
}

/// `GetAvailableCoreWebView2BrowserVersionStringWithOptions` del cargador: la versión del runtime
/// que se usaría, en memoria de COM (la libera quien llama, con `CoTaskMemFree`). Las opciones
/// solo eligen canales de prueba, que no se buscan.
///
/// # Safety
/// `browser_folder`, nula o terminada en NUL; `version_info`, un `PWSTR` escribible.
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn GetAvailableCoreWebView2BrowserVersionStringWithOptions(
    browser_folder: PCWSTR,
    _options: *mut c_void,
    version_info: *mut PWSTR,
) -> HRESULT {
    if version_info.is_null() {
        return E_POINTER;
    }
    // SAFETY: el contrato de esta función.
    unsafe { *version_info = PWSTR::null() };
    // SAFETY: el contrato de esta función.
    let folder = unsafe { param(browser_folder) };
    let version = match find_runtime_for(folder.as_deref().map(Path::new)) {
        Ok(runtime) => match runtime.version() {
            Some(v) => v,
            None => return E_FAIL,
        },
        Err(e) => {
            log::debug!("WebView2: {e}");
            return not_found();
        }
    };
    let wide: Vec<u16> = version.to_string().encode_utf16().chain([0]).collect();
    // SAFETY: memoria de COM del tamaño de la cadena, que se copia entera.
    unsafe {
        let p = CoTaskMemAlloc(wide.len() * 2).cast::<u16>();
        if p.is_null() {
            return E_OUTOFMEMORY;
        }
        p.copy_from_nonoverlapping(wide.as_ptr(), wide.len());
        *version_info = PWSTR(p);
    }
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("versión {s}"))
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(v("157.0.4341.46"), Version([157, 0, 4341, 46]));
        assert_eq!(v("1.0"), Version([1, 0, 0, 0]));
        assert_eq!(v(" 78.0.276.0 dev"), Version([78, 0, 276, 0]), "canal");
        assert_eq!(v("157.0.4341.46").to_string(), "157.0.4341.46");
        for bad in [
            "",
            "dev",
            "a.b",
            "1..2",
            "1.2.3.4.5",
            "1.-2",
            "99999999999.0",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert!(v("157.0.4341.46") > v("86.0.616.0"));
        assert!(v("100.0.0.9") < v("100.0.1.0"), "numérico, no de texto");
    }

    /// Instalaciones de mentira: (dónde, EBWebView, pv, location).
    fn install(
        hive: Hive,
        ebwebview: Option<&str>,
        pv: Option<&str>,
        location: Option<&str>,
    ) -> Install {
        Install {
            hive,
            ebwebview: ebwebview.map(str::to_owned),
            pv: pv.map(str::to_owned),
            location: location.map(str::to_owned),
        }
    }

    const MACHINE_APP: &str = r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application";
    const USER_APP: &str = r"C:\Users\ana\AppData\Local\Microsoft\EdgeWebView\Application";

    fn dll(folder: &str) -> PathBuf {
        client_dll(Path::new(folder))
    }

    #[test]
    fn client_dll_path_inside_the_version_folder() {
        let arch = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86"
        };
        assert_eq!(
            dll(r"C:\X\Application\157.0.4341.46"),
            PathBuf::from(format!(
                r"C:\X\Application\157.0.4341.46\EBWebView\{arch}\EmbeddedBrowserWebView.dll"
            ))
        );
    }

    #[test]
    fn highest_valid_version_wins_between_machine_and_user() {
        let all = |_: &Path| true;
        let machine_old = install(Hive::Machine, None, Some("150.0.1.2"), Some(MACHINE_APP));
        let user_new = install(Hive::User, None, Some("157.0.4341.46"), Some(USER_APP));
        let got = pick_installed(&[machine_old.clone(), user_new.clone()], all).expect("runtime");
        assert_eq!(
            got,
            Runtime {
                dll: dll(&format!(r"{USER_APP}\157.0.4341.46")),
                kind: RuntimeKind::Installed(v("157.0.4341.46")),
            }
        );

        let machine_new = install(
            Hive::Machine,
            None,
            Some("157.0.4341.46"),
            Some(MACHINE_APP),
        );
        let user_old = install(Hive::User, None, Some("150.0.1.2"), Some(USER_APP));
        let got = pick_installed(&[user_old, machine_new.clone()], all).expect("runtime");
        assert_eq!(got.dll, dll(&format!(r"{MACHINE_APP}\157.0.4341.46")));

        // misma versión en los dos: la del equipo
        let user_same = install(Hive::User, None, Some("157.0.4341.46"), Some(USER_APP));
        for order in [
            [user_same.clone(), machine_new.clone()],
            [machine_new.clone(), user_same.clone()],
        ] {
            let got = pick_installed(&order, all).expect("runtime");
            assert_eq!(got.dll, dll(&format!(r"{MACHINE_APP}\157.0.4341.46")));
        }
    }

    #[test]
    fn ebwebview_value_gives_the_version_folder() {
        let folder = format!(r"{MACHINE_APP}\157.0.4341.46");
        let only_state = install(Hive::Machine, Some(&folder), None, None);
        let got = pick_installed(&[only_state], |_| true).expect("runtime");
        assert_eq!(
            got,
            Runtime {
                dll: dll(&folder),
                kind: RuntimeKind::Installed(v("157.0.4341.46")),
            }
        );
        // si ClientState y Clients no coinciden (a mitad de una actualización), la más alta
        let newer = format!(r"{MACHINE_APP}\158.0.1.0");
        let mixed = install(
            Hive::Machine,
            Some(&newer),
            Some("157.0.4341.46"),
            Some(MACHINE_APP),
        );
        assert_eq!(
            pick_installed(&[mixed], |_| true).map(|r| r.dll),
            Some(dll(&newer))
        );
    }

    #[test]
    fn invalid_or_missing_entries_are_skipped() {
        let good = format!(r"{USER_APP}\140.0.1.1");
        let exists = |p: &Path| p == dll(&good);
        let installs = [
            // la DLL no está (desinstalado a medias)
            install(
                Hive::Machine,
                None,
                Some("157.0.4341.46"),
                Some(MACHINE_APP),
            ),
            // versión que no se entiende, carpeta vacía o relativa, falta un valor
            install(
                Hive::Machine,
                Some(r"C:\X\basura"),
                Some("basura"),
                Some(MACHINE_APP),
            ),
            install(Hive::Machine, Some(""), Some("150.0.0.0"), Some("")),
            install(
                Hive::Machine,
                Some(r"relativa\150.0.0.0"),
                Some("150.0.0.0"),
                None,
            ),
            install(Hive::Machine, None, None, Some(MACHINE_APP)),
            // anterior a la mínima (86.0.616.0)
            install(Hive::Machine, None, Some("85.0.564.0"), Some(MACHINE_APP)),
            install(Hive::User, None, Some("140.0.1.1"), Some(USER_APP)),
        ];
        let everything = |p: &Path| p.is_absolute();
        assert_eq!(
            pick_installed(&installs, exists).map(|r| r.dll),
            Some(dll(&good))
        );
        // con todas las DLL "presentes", gana la 157 (la 85 no cuenta aunque esté)
        assert_eq!(
            pick_installed(&installs, everything).map(|r| r.kind),
            Some(RuntimeKind::Installed(v("157.0.4341.46")))
        );
        let too_old = install(Hive::Machine, None, Some("85.0.564.0"), Some(MACHINE_APP));
        assert_eq!(pick_installed(&[too_old], everything), None);
    }

    #[test]
    fn nothing_installed_is_a_clear_spanish_error() {
        let empty = [
            install(Hive::Machine, None, None, None),
            install(Hive::User, None, None, None),
        ];
        assert_eq!(choose(None, &empty, |_| true), Err(NOT_FOUND.to_string()));
        assert_eq!(choose(None, &[], |_| true), Err(NOT_FOUND.to_string()));
        assert_eq!(
            NOT_FOUND,
            "No se encontró WebView2. Instálalo desde https://go.microsoft.com/fwlink/p/?LinkId=2124703"
        );
    }

    #[test]
    fn fixed_folder_env_var_beats_the_parameter() {
        let exe_dir = Path::new(r"C:\Apps\Anotalo");
        let env = OsString::from(r"D:\WebView2Fijo");
        let param = Path::new(r"E:\DelParametro");
        assert_eq!(
            fixed_folder(Some(&env), Some(param), exe_dir),
            Some(PathBuf::from(r"D:\WebView2Fijo"))
        );
        assert_eq!(
            fixed_folder(Some(OsStr::new("")), Some(param), exe_dir),
            Some(param.to_path_buf()),
            "vacía no cuenta"
        );
        assert_eq!(fixed_folder(None, None, exe_dir), None);
        assert_eq!(
            fixed_folder(Some(OsStr::new("runtime")), None, exe_dir),
            Some(PathBuf::from(r"C:\Apps\Anotalo\runtime")),
            "relativa, junto al exe"
        );
    }

    #[test]
    fn fixed_folder_is_used_alone() {
        let fixed = Path::new(r"D:\WebView2Fijo");
        let installed = install(
            Hive::Machine,
            None,
            Some("157.0.4341.46"),
            Some(MACHINE_APP),
        );
        assert_eq!(
            choose(Some(fixed), std::slice::from_ref(&installed), |_| true),
            Ok(Runtime {
                dll: client_dll(fixed),
                kind: RuntimeKind::Fixed,
            })
        );
        // si en la carpeta fija no está, no se usa el instalado: se dice dónde se buscó
        let only_installed = |p: &Path| p != client_dll(fixed);
        let err = choose(Some(fixed), &[installed], only_installed).unwrap_err();
        assert!(err.contains(r"D:\WebView2Fijo"), "{err}");
        assert!(err.starts_with("No se encontró WebView2"), "{err}");
    }

    #[test]
    fn user_data_folder_override_and_default() {
        let env = OsString::from(r"D:\Datos");
        let param = Path::new(r"C:\Users\ana\AppData\Local\Anotalo\WebView2");
        assert_eq!(
            user_data_folder(Some(&env), Some(param)),
            Some(PathBuf::from(r"D:\Datos"))
        );
        assert_eq!(
            user_data_folder(Some(OsStr::new("")), Some(param)),
            Some(param.to_path_buf())
        );
        assert_eq!(user_data_folder(None, None), None);
        assert_eq!(
            default_user_data_folder(Path::new(r"C:\Apps\anotalo.exe")),
            PathBuf::from(r"C:\Apps\anotalo.exe.WebView2")
        );
    }

    /// Un `VS_VERSIONINFO` mínimo: cabecera, clave, relleno y `VS_FIXEDFILEINFO`.
    fn version_resource(product: Version) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&[0; 6]); // wLength, wValueLength, wType
        for c in "VS_VERSION_INFO\0".encode_utf16() {
            r.extend_from_slice(&c.to_le_bytes());
        }
        r.extend_from_slice(&[0; 2]); // relleno hasta 4
        let [a, b, c, d] = product.0;
        let ms = |hi: u32, lo: u32| ((hi << 16) | lo).to_le_bytes();
        r.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        r.extend_from_slice(&0x0001_0000u32.to_le_bytes());
        r.extend_from_slice(&ms(9, 9)); // versión de archivo: no es la que vale
        r.extend_from_slice(&ms(9, 9));
        r.extend_from_slice(&ms(a, b));
        r.extend_from_slice(&ms(c, d));
        r.extend_from_slice(&[0; 28]);
        r
    }

    #[test]
    fn product_version_from_the_version_resource() {
        let res = version_resource(v("157.0.4341.46"));
        assert_eq!(fixed_file_version(&res), Some(v("157.0.4341.46")));
        assert_eq!(fixed_file_version(&res[..50]), None, "cortado");
        assert_eq!(fixed_file_version(&[0; 100]), None, "sin firma");
    }

    fn compare(a: PCWSTR, b: PCWSTR) -> (HRESULT, i32) {
        let mut result = 7;
        // SAFETY: cadenas literales y un i32 local.
        let hr = unsafe { CompareBrowserVersions(a, b, &mut result) };
        (hr, result)
    }

    #[test]
    fn compare_browser_versions_like_the_sdk() {
        assert_eq!(compare(w!("1.0.0"), w!("1.0.0")), (S_OK, 0));
        assert_eq!(compare(w!("1.0.0"), w!("1.0.1")), (S_OK, -1));
        assert_eq!(compare(w!("2.0.0"), w!("1.0.1")), (S_OK, 1));
        assert_eq!(compare(w!("78.0.276.0 dev"), w!("78.0.276.0")), (S_OK, 0));
        assert_eq!(compare(w!("nada"), w!("1.0")).0, E_INVALIDARG);
        assert_eq!(compare(PCWSTR::null(), w!("1.0")).0, E_INVALIDARG);
    }

    /// La versión que da `GetAvailableCoreWebView2BrowserVersionString` para esa carpeta.
    fn available_version(folder: PCWSTR) -> Result<String, HRESULT> {
        let mut out = PWSTR::null();
        // SAFETY: carpeta terminada en NUL (o nula) y un PWSTR local, que se libera aquí.
        unsafe {
            let hr = GetAvailableCoreWebView2BrowserVersionString(folder, &mut out);
            if hr != S_OK {
                return Err(hr);
            }
            let version = out.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(out.0 as *const c_void));
            Ok(version)
        }
    }

    #[test]
    #[ignore = "necesita WebView2 instalado"]
    fn installed_runtime_is_found_with_its_entry_point() {
        let runtime = find_runtime().expect("WebView2 instalado");
        let RuntimeKind::Installed(version) = runtime.kind else {
            panic!("se esperaba el instalado: {runtime:?}")
        };
        assert!(version >= MIN_VERSION, "{version}");
        assert!(runtime.dll.is_file(), "{}", runtime.dll.display());
        // la versión de producto de la DLL es la del registro (así se lee la de una carpeta fija)
        assert_eq!(dll_version(&runtime.dll), Some(version));
        assert_eq!(available_version(PCWSTR::null()), Ok(version.to_string()));

        // la carpeta de la versión vale también como carpeta fija
        let folder = runtime
            .dll
            .ancestors()
            .nth(3)
            .expect("carpeta de la versión");
        let fixed = choose(Some(folder), &[], Path::is_file).expect("carpeta fija");
        assert_eq!(fixed.dll, runtime.dll);
        assert_eq!(fixed.version(), Some(version));
        let wide = HSTRING::from(folder);
        assert_eq!(
            available_version(PCWSTR(wide.as_ptr())),
            Ok(version.to_string())
        );

        // y la DLL tiene la función que llamamos
        let dll = HSTRING::from(runtime.dll.as_path());
        // SAFETY: como en `create_environment`; aquí se suelta al final.
        unsafe {
            let module = LoadLibraryExW(
                &dll,
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .expect("cargar EmbeddedBrowserWebView.dll");
            let create = GetProcAddress(module, s!("CreateWebViewEnvironmentWithOptionsInternal"));
            assert!(
                create.is_some(),
                "falta CreateWebViewEnvironmentWithOptionsInternal"
            );
            let _ = FreeLibrary(module);
        }
    }

    #[test]
    fn version_string_of_a_missing_fixed_folder_is_not_found() {
        let mut version = PWSTR(w!("x").as_ptr().cast_mut());
        // SAFETY: carpeta literal y un PWSTR local.
        let hr = unsafe {
            GetAvailableCoreWebView2BrowserVersionString(w!(r"C:\no\existe\webview2"), &mut version)
        };
        assert_eq!(hr, ERROR_FILE_NOT_FOUND.to_hresult());
        assert!(version.is_null());
    }
}

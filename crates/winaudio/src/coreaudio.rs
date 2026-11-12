//! Dispositivos y sesiones de audio de Windows (Core Audio), leídos en el momento (port de
//! calltx/audio/coreaudio.py).
//!
//! Para cada dispositivo activo, qué programas tienen sonido abierto (y si está sonando o grabando
//! ahora). Así se sabe en qué auriculares suena la llamada o qué micrófono usa, y también si una app
//! de llamadas usa el micrófono aunque el registro de privacidad de Windows no lo anote.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anotalo_core::routing::{SessionInfo, platform_for_key, process_key};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture,
    eCommunications, eConsole, eRender,
};
use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::core::{Interface, PWSTR};

/// Los dispositivos activos de un sentido (salida o entrada) y quién tiene sonido abierto en ellos.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub sessions: Vec<SessionInfo>,
    /// Id del dispositivo → nombre.
    pub names: HashMap<String, String>,
    /// Predeterminado de comunicaciones.
    pub comm: Option<String>,
    /// Predeterminado general.
    pub console: Option<String>,
}

pub fn render_snapshot() -> Result<Snapshot, String> {
    in_mta(|| snapshot(eRender))
}

pub fn capture_snapshot() -> Result<Snapshot, String> {
    in_mta(|| snapshot(eCapture))
}

/// Programas grabando de algún micrófono ahora mismo (claves como las del registro).
pub fn capture_keys() -> Result<HashSet<String>, String> {
    Ok(capture_snapshot()?
        .sessions
        .into_iter()
        .filter(|s| s.active)
        .filter_map(|s| s.key)
        .collect())
}

/// Clave del programa (como en el registro del micrófono): su paquete; si no, su exe. Los procesos
/// auxiliares sin nombre propio (p. ej. WebView2) toman la de su app padre si esa es una plataforma
/// de llamadas (hasta 4 niveles).
pub fn key_for_pid(pid: u32) -> Option<String> {
    key_in(&processes(), pid)
}

/// COM iniciado en este hilo mientras viva.
struct Com;

impl Com {
    fn init_mta() -> Result<Com, String> {
        // SAFETY: se empareja con CoUninitialize en Drop, en el mismo hilo.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| format!("no se pudo iniciar COM: {e}"))?;
        Ok(Com)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: este hilo inició COM en `init_mta`.
        unsafe { CoUninitialize() }
    }
}

/// Core Audio necesita COM: se usa en un hilo propio (quien llama puede ser el hilo de la ventana).
fn in_mta<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("coreaudio".into())
        .spawn(move || {
            let result = Com::init_mta().and_then(|_com| f());
            let _ = tx.send(result);
        })
        .map_err(|e| format!("no se pudo arrancar el hilo de Core Audio: {e}"))?;
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err("Core Audio no respondió".into()),
        Err(RecvTimeoutError::Disconnected) => Err("Core Audio falló".into()),
    }
}

fn snapshot(flow: EDataFlow) -> Result<Snapshot, String> {
    let err = |e: windows::core::Error| format!("Core Audio: {e}");
    // SAFETY: llamadas COM en un hilo con COM iniciado; las cadenas devueltas se liberan al leerlas.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
        let mut snap = Snapshot {
            comm: default_id(&enumerator, flow, eCommunications),
            console: default_id(&enumerator, flow, eConsole),
            ..Snapshot::default()
        };
        let devices = enumerator
            .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
            .map_err(err)?;
        let procs = processes();
        let mut keys: HashMap<u32, Option<String>> = HashMap::new();
        for i in 0..devices.GetCount().map_err(err)? {
            let Ok(dev) = devices.Item(i) else { continue };
            let Some(id) = device_id(&dev) else { continue };
            let name = device_name(&dev).unwrap_or_else(|| id.clone());
            snap.names.insert(id.clone(), name.clone());
            let sessions = match dev
                .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                .and_then(|m| m.GetSessionEnumerator())
            {
                Ok(s) => s,
                Err(e) => {
                    log::debug!("Sin sesiones en {name}: {e}");
                    continue;
                }
            };
            for j in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(ctl) = sessions.GetSession(j) else {
                    continue;
                };
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let (Ok(pid), Ok(state)) = (ctl2.GetProcessId(), ctl.GetState()) else {
                    continue;
                };
                if pid == 0 {
                    continue; // sonidos del sistema
                }
                let key = keys
                    .entry(pid)
                    .or_insert_with(|| key_in(&procs, pid))
                    .clone();
                snap.sessions.push(SessionInfo {
                    endpoint_id: id.clone(),
                    endpoint_name: name.clone(),
                    pid,
                    active: state == AudioSessionStateActive,
                    key,
                });
            }
        }
        Ok(snap)
    }
}

/// Lee y libera una cadena reservada por COM.
unsafe fn take_string(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: `p` es una cadena terminada en NUL reservada con CoTaskMemAlloc.
    unsafe {
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

unsafe fn device_id(dev: &IMMDevice) -> Option<String> {
    // SAFETY: ver `snapshot`.
    unsafe { take_string(dev.GetId().ok()?) }
}

unsafe fn device_name(dev: &IMMDevice) -> Option<String> {
    // SAFETY: ver `snapshot`; el PROPVARIANT se libera con PropVariantClear.
    unsafe {
        let store = dev.OpenPropertyStore(STGM_READ).ok()?;
        let mut pv = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let name = PropVariantToStringAlloc(&pv)
            .ok()
            .and_then(|p| take_string(p));
        let _ = PropVariantClear(&mut pv);
        name.filter(|n| !n.is_empty())
    }
}

unsafe fn default_id(
    enumerator: &IMMDeviceEnumerator,
    flow: EDataFlow,
    role: ERole,
) -> Option<String> {
    // SAFETY: ver `snapshot`. Falla si no hay dispositivos de ese tipo.
    unsafe { device_id(&enumerator.GetDefaultAudioEndpoint(flow, role).ok()?) }
}

struct Proc {
    parent: u32,
    exe: String,
}

/// Todos los procesos: su padre y el nombre de su exe.
fn processes() -> HashMap<u32, Proc> {
    let mut out = HashMap::new();
    // SAFETY: instantánea de procesos de ToolHelp, recorrida y cerrada aquí.
    unsafe {
        let Ok(h) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(h, &mut e).is_ok();
        while more {
            out.insert(
                e.th32ProcessID,
                Proc {
                    parent: e.th32ParentProcessID,
                    exe: from_wide(&e.szExeFile),
                },
            );
            more = Process32NextW(h, &mut e).is_ok();
        }
        let _ = CloseHandle(h);
    }
    out
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Nombre de familia del paquete de una app empaquetada (Teams, WhatsApp…), o None.
fn package_family(pid: u32) -> Option<String> {
    // SAFETY: el handle se abre y se cierra aquí; el búfer tiene la longitud que pide Windows.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut len = 0u32;
        let mut name = None;
        if GetPackageFamilyName(h, &mut len, None) == ERROR_INSUFFICIENT_BUFFER && len > 0 {
            let mut buf = vec![0u16; len as usize];
            if GetPackageFamilyName(h, &mut len, Some(PWSTR(buf.as_mut_ptr()))) == ERROR_SUCCESS {
                name = Some(from_wide(&buf)).filter(|s| !s.is_empty());
            }
        }
        let _ = CloseHandle(h);
        name
    }
}

fn key_in(procs: &HashMap<u32, Proc>, pid: u32) -> Option<String> {
    let me = procs.get(&pid)?;
    let is_call = |k: &Option<String>| k.as_deref().is_some_and(|k| platform_for_key(k).is_some());
    let key = process_key(Some(&me.exe), package_family(pid).as_deref());
    if is_call(&key) {
        return key;
    }
    let mut cur = me.parent;
    for _ in 0..4 {
        if cur == 0 || cur == pid {
            break;
        }
        let Some(p) = procs.get(&cur) else { break };
        let k = process_key(Some(&p.exe), package_family(cur).as_deref());
        if is_call(&k) {
            return k;
        }
        cur = p.parent;
    }
    key
}

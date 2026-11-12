//! Elección de las fuentes de audio de una llamada (port de calltx/audio/sources.py).

use std::collections::HashMap;

use anotalo_core::routing::{Audio, pick_capture, pick_render};
use anotalo_core::source::{AudioSource, OthersSpec};

use crate::capture::{EndpointLoopbackSource, MicSource, ProcessLoopbackSource};
use crate::coreaudio::{self, Snapshot};

/// De dónde se escuchará a "los demás".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OthersChoice {
    /// El audio del programa (y sus hijos).
    Process(u32),
    /// Todo lo que suena en esa salida.
    Endpoint { id: String, name: String },
    /// La salida predeterminada (Core Audio no respondió).
    DefaultOutput,
}

/// PID del proceso raíz de una app multiproceso (p. ej. el firefox.exe padre). `procs`: (pid, exe,
/// pid del padre).
pub fn root_pid_in(procs: &[(u32, String, u32)], exe: &str) -> Option<u32> {
    let exe = exe.to_lowercase();
    let names: HashMap<u32, String> = procs
        .iter()
        .map(|(pid, name, _)| (*pid, name.to_lowercase()))
        .collect();
    procs
        .iter()
        .filter(|(_, name, parent)| {
            name.to_lowercase() == exe && names.get(parent).is_none_or(|p| *p != exe)
        })
        .map(|(pid, _, _)| *pid)
        .min()
}

pub fn find_root_pid(exe: &str) -> Option<u32> {
    root_pid_in(&coreaudio::process_list(), exe)
}

/// De dónde sale la voz de "los demás". Se decide en el momento (y otra vez si la fuente se cae):
/// el audio del programa si se puede; si no, la salida donde suena la llamada.
pub fn decide_others(
    spec: &OthersSpec,
    root_pid: &dyn Fn(&str) -> Option<u32>,
    probe: &dyn Fn(u32) -> Result<(), String>,
    render: &dyn Fn() -> Result<Snapshot, String>,
) -> OthersChoice {
    if spec.kind == Audio::Process
        && let Some(exe) = spec.exe.as_deref()
        && let Some(pid) = root_pid(exe)
    {
        match probe(pid) {
            Ok(()) => return OthersChoice::Process(pid),
            Err(e) => log::warn!("Sin audio del proceso {exe} ({e}); uso su dispositivo de salida"),
        }
    }
    let snap = match render() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("No se pudo leer el audio de Windows ({e}); uso la salida predeterminada");
            return OthersChoice::DefaultOutput;
        }
    };
    let (ep, why) = pick_render(
        &snap.sessions,
        spec.platform.as_deref(),
        snap.comm.as_deref(),
        snap.console.as_deref(),
    );
    let Some(id) = ep else {
        return OthersChoice::DefaultOutput;
    };
    let name = snap.names.get(&id).cloned().unwrap_or_default();
    log::info!(
        "Otros: {} ({why})",
        if name.is_empty() { &id } else { &name }
    );
    OthersChoice::Endpoint { id, name }
}

/// Micrófono para "Yo" (id del dispositivo; None = el predeterminado): el elegido en Ajustes (por
/// su id o su nombre, como lo guardaba la app de Python); si no, el que usa la app de la llamada; si
/// no, el predeterminado de este momento.
pub fn decide_mic(
    platform: Option<&str>,
    mic_setting: Option<&str>,
    capture: &dyn Fn() -> Result<Snapshot, String>,
) -> Option<String> {
    let snap = match capture() {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "No se pudo leer el audio de Windows ({e}); uso el micrófono predeterminado"
            );
            return None;
        }
    };
    if let Some(want) = mic_setting.filter(|s| !s.is_empty()) {
        if let Some(id) = find_named(&snap, want) {
            log::info!("Yo: {} (elegido en Ajustes)", snap.names[&id]);
            return Some(id);
        }
        log::warn!("No encuentro el micrófono «{want}»; uso el de la llamada");
    }
    let (ep, why) = pick_capture(&snap.sessions, platform, snap.console.as_deref());
    let name = ep.as_ref().and_then(|e| snap.names.get(e));
    log::info!(
        "Yo: {} ({why})",
        name.map_or("predeterminado", String::as_str)
    );
    ep
}

/// El dispositivo con ese id; si no, el de ese nombre; si no, el primero cuyo nombre lo contenga
/// (Windows recortaba los nombres largos en la app de Python).
fn find_named(snap: &Snapshot, want: &str) -> Option<String> {
    if snap.names.contains_key(want) {
        return Some(want.to_string());
    }
    let want = want.to_lowercase();
    let mut ids: Vec<&String> = snap.names.keys().collect();
    ids.sort();
    let named = |exact: bool| {
        ids.iter()
            .find(|id| {
                let n = snap.names[**id].to_lowercase();
                if exact { n == want } else { n.contains(&want) }
            })
            .map(|id| (*id).clone())
    };
    named(true).or_else(|| named(false))
}

pub fn pick_others(spec: &OthersSpec) -> Box<dyn AudioSource> {
    match decide_others(
        spec,
        &find_root_pid,
        &ProcessLoopbackSource::probe_pid,
        &coreaudio::render_snapshot,
    ) {
        OthersChoice::Process(pid) => Box::new(ProcessLoopbackSource::new(pid)),
        OthersChoice::Endpoint { id, name } => {
            Box::new(EndpointLoopbackSource::new(Some(id), name))
        }
        OthersChoice::DefaultOutput => Box::new(EndpointLoopbackSource::new(None, String::new())),
    }
}

pub fn pick_mic(platform: Option<&str>, mic_setting: Option<&str>) -> Box<dyn AudioSource> {
    Box::new(MicSource::new(decide_mic(
        platform,
        mic_setting,
        &coreaudio::capture_snapshot,
    )))
}

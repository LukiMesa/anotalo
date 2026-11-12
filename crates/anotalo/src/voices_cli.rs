//! `anotalo nombrar` y `anotalo voces`: poner nombre a las personas de una llamada y ver o borrar
//! las voces guardadas, hasta que la ventana (M3) tenga la biblioteca y los ajustes.

use std::path::{Path, PathBuf};

use anotalo_core::config::{config_path, load_settings};
use anotalo_core::paths::{self, Profile};
use anotalo_core::speaker::memory::{VoiceStore, voices_path};
use anotalo_core::speaker::naming::name_in_call;

/// `%APPDATA%\Anotalo\llamadas`: las voces de cada llamada.
pub fn prints_dir(p: Profile) -> PathBuf {
    paths::data_dir(p).join("llamadas")
}

pub fn name(file: &Path, from: &str, to: &str) -> Result<String, String> {
    let p = paths::profile();
    let mut store = VoiceStore::load(&voices_path(p));
    let s = load_settings(&config_path(p), p);
    let reserved = [s.label_me.as_str(), s.label_others.as_str()];
    let out = name_in_call(file, &prints_dir(p), &mut store, from, to, &reserved)?;
    let plural = if out.lines == 1 { "línea" } else { "líneas" };
    let mut msg = format!(
        "Renombré {} {plural} de «{from}» a «{}».",
        out.lines,
        to.trim()
    );
    if out.remembered {
        msg.push_str(" Recordaré su voz.");
    }
    Ok(msg)
}

pub fn list() -> String {
    let store = VoiceStore::load(&voices_path(paths::profile()));
    if store.voices.is_empty() {
        return "No hay voces guardadas.".into();
    }
    store
        .voices
        .iter()
        .map(|v| {
            let min = ((v.seconds / 60.0).round() as u64).max(1);
            format!("{} — {min} min de voz", v.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn forget(name: &str) -> Result<String, String> {
    let mut store = VoiceStore::load(&voices_path(paths::profile()));
    if !store.forget(name) {
        return Err(format!("No hay ninguna voz guardada como «{name}»."));
    }
    store
        .save()
        .map_err(|e| format!("No se pudo guardar: {e}"))?;
    Ok(format!("Borré la voz de «{name}»."))
}

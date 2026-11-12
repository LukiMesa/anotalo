//! Memoria de voces (spec §2.4.5): las huellas de las personas que el usuario ha nombrado, en
//! `%APPDATA%\Anotalo\voces.json`, solo en este PC. Al colgar, un grupo recibe un nombre guardado
//! solo si la mayoría de sus segmentos se le parece (umbral conservador): nunca se inventa un
//! nombre equivocado.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::embed::{cosine, l2n};
use crate::paths::{Profile, data_dir};

/// Un segmento se parece a una voz guardada si el coseno (huellas centradas) llega a esto.
pub const VOICE_MATCH: f32 = 0.44;
/// Votan los segmentos así de largos (todos, si el grupo no tiene ninguno).
pub const VOTE_MIN_S: f64 = 2.0;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StoredVoice {
    pub name: String,
    /// Huella centrada y de norma 1.
    pub print: Vec<f32>,
    /// Segundos de voz con los que se hizo la huella.
    pub seconds: f64,
}

#[derive(Serialize, Deserialize)]
struct VoicesFile {
    version: u32,
    voices: Vec<StoredVoice>,
}

pub struct VoiceStore {
    pub voices: Vec<StoredVoice>,
    path: PathBuf,
}

/// `%APPDATA%\Anotalo\voces.json` (o el de la versión de desarrollo).
pub fn voices_path(p: Profile) -> PathBuf {
    data_dir(p).join("voces.json")
}

impl VoiceStore {
    /// Nunca falla: sin archivo, vacía. Un archivo que no se entiende se copia a `voces.json.bad`
    /// antes de que el siguiente guardado lo sobrescriba.
    pub fn load(path: &Path) -> VoiceStore {
        let empty = || VoiceStore {
            voices: Vec::new(),
            path: path.to_path_buf(),
        };
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return empty(),
            Err(e) => {
                log::warn!("No se pudieron leer las voces guardadas ({e})");
                return empty();
            }
        };
        match serde_json::from_slice::<VoicesFile>(&bytes) {
            Ok(f) => VoiceStore {
                voices: f.voices,
                path: path.to_path_buf(),
            },
            Err(e) => {
                let bad = path.with_extension("json.bad");
                let copied = fs::write(&bad, &bytes).is_ok();
                log::warn!(
                    "Voces guardadas dañadas ({e}); {}",
                    if copied {
                        format!("copia en {}", bad.display())
                    } else {
                        "no se pudo copiar".into()
                    }
                );
                empty()
            }
        }
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = VoicesFile {
            version: 1,
            voices: self.voices.clone(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &self.path)
    }

    /// Guarda la huella de `name`; si ya había una, la mezcla con la nueva según los segundos.
    pub fn remember(&mut self, name: &str, print: &[f32], seconds: f64) {
        match self.voices.iter_mut().find(|v| v.name == name) {
            Some(v) => {
                v.print = mix(&v.print, v.seconds, print, seconds);
                v.seconds += seconds;
            }
            None => self.voices.push(StoredVoice {
                name: name.to_string(),
                print: l2n(print),
                seconds,
            }),
        }
    }

    pub fn forget(&mut self, name: &str) -> bool {
        let before = self.voices.len();
        self.voices.retain(|v| v.name != name);
        self.voices.len() != before
    }

    /// Cambia el nombre de una voz; si `to` ya existía, se mezclan.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let i = self
            .voices
            .iter()
            .position(|v| v.name == from)
            .ok_or_else(|| format!("No hay ninguna voz guardada como «{from}»"))?;
        if from == to {
            return Ok(());
        }
        let v = self.voices.remove(i);
        self.remember(to, &v.print, v.seconds);
        Ok(())
    }
}

fn mix(a: &[f32], a_s: f64, b: &[f32], b_s: f64) -> Vec<f32> {
    let v: Vec<f32> = a
        .iter()
        .zip(b)
        .map(|(x, y)| (f64::from(*x) * a_s + f64::from(*y) * b_s) as f32)
        .collect();
    l2n(&v)
}

/// El nombre guardado de cada grupo (uno por llamada), o None. Cada grupo es una lista de
/// (huella centrada, segundos).
pub fn name_clusters(
    clusters: &[Vec<(Vec<f32>, f64)>],
    voices: &[StoredVoice],
) -> Vec<Option<String>> {
    // (nombre, proporción de votos, segundos del grupo)
    let votes: Vec<Option<(usize, f64, f64)>> = clusters.iter().map(|c| vote(c, voices)).collect();
    votes
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let (who, share, secs) = (*v)?;
            let beaten = votes.iter().enumerate().any(|(j, o)| {
                j != i
                    && o.is_some_and(|(w, s, t)| {
                        w == who
                            && (s > share || (s == share && (t > secs || (t == secs && j < i))))
                    })
            });
            (!beaten).then(|| voices[who].name.clone())
        })
        .collect()
}

/// La voz a la que vota al menos la mitad de los segmentos que votan: (índice, proporción, segundos).
fn vote(cluster: &[(Vec<f32>, f64)], voices: &[StoredVoice]) -> Option<(usize, f64, f64)> {
    if voices.is_empty() || cluster.is_empty() {
        return None;
    }
    let long: Vec<&(Vec<f32>, f64)> = cluster.iter().filter(|(_, s)| *s >= VOTE_MIN_S).collect();
    let voters: Vec<&(Vec<f32>, f64)> = if long.is_empty() {
        cluster.iter().collect()
    } else {
        long
    };
    let mut counts = vec![0usize; voices.len()];
    for (e, _) in &voters {
        let (best, sim) = voices
            .iter()
            .map(|v| cosine(e, &v.print))
            .enumerate()
            .fold((0, f32::MIN), |b, (i, s)| if s > b.1 { (i, s) } else { b });
        if sim >= VOICE_MATCH {
            counts[best] += 1;
        }
    }
    let (who, n) = counts
        .iter()
        .copied()
        .enumerate()
        .fold((0, 0), |b, (i, n)| if n > b.1 { (i, n) } else { b });
    if n == 0 || (n as f64) < 0.5 * voters.len() as f64 {
        return None;
    }
    let secs = cluster.iter().map(|(_, s)| s).sum();
    Some((who, n as f64 / voters.len() as f64, secs))
}

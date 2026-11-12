//! Memoria de voces (spec §2.4.5): las huellas de las personas que el usuario ha nombrado, en
//! `%APPDATA%\Anotalo\voces.json`, solo en este PC. Al colgar, un grupo recibe un nombre guardado
//! solo si la mayoría de sus segmentos se le parece (umbral conservador): nunca se inventa un
//! nombre equivocado.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::embed::{cosine, l2n};
use crate::paths::{Profile, data_dir};

/// Un segmento se parece a una voz guardada si el coseno (huellas centradas) llega a esto. El
/// spike daba 0,44; con 0,50 nada cambia en audio de llamada y en audio limpio salen con nombre
/// la mitad de desconocidos (5,6 % frente a 9,7 %).
pub const VOICE_MATCH: f32 = 0.50;
/// Un grupo con menos segmentos que voten no recibe nombre (uno solo no basta).
pub const MIN_VOTERS: usize = 2;
/// Votan los segmentos así de largos (todos, si el grupo no tiene ninguno).
pub const VOTE_MIN_S: f64 = 2.0;
/// El espacio de las huellas: el modelo que las saca (TitaNet small) y la media de fondo con la
/// que se centran (`titanet_small_mu.bin`, de audio degradado). Huellas de espacios distintos no
/// se pueden comparar: si esto cambia, las guardadas con el anterior dejan de usarse.
pub const PRINT_SPACE: &str = "titanet-small/mu-deg-1";
/// Un `.tmp` sin tocar desde hace esto es de un guardado que no terminó.
pub const STALE_TMP: Duration = Duration::from_secs(86_400);

/// Los archivos sin espacio de huellas solo los escribieron versiones de desarrollo, todas con el
/// actual.
pub(crate) fn current_space() -> String {
    PRINT_SPACE.to_string()
}

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
    #[serde(default = "current_space")]
    space: String,
    voices: Vec<StoredVoice>,
}

pub struct VoiceStore {
    pub voices: Vec<StoredVoice>,
    path: PathBuf,
    /// false si el archivo existía y no se pudo leer: guardar encima perdería las voces.
    writable: bool,
}

/// `%APPDATA%\Anotalo\voces.json` (o el de la versión de desarrollo).
pub fn voices_path(p: Profile) -> PathBuf {
    data_dir(p).join("voces.json")
}

impl VoiceStore {
    /// Nunca falla: sin archivo, vacía. Un archivo que no se entiende se copia a `voces.json.bad`
    /// antes de que el siguiente guardado lo sobrescriba; uno de otro espacio de huellas se aparta
    /// a `voces.json.<espacio>.bak`. Las dos copias se borran a los 30 días
    /// ([`prune_voice_backups`]).
    pub fn load(path: &Path) -> VoiceStore {
        let empty = |writable: bool| VoiceStore {
            voices: Vec::new(),
            path: path.to_path_buf(),
            writable,
        };
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return empty(true),
            Err(e) => {
                log::warn!("No se pudieron leer las voces guardadas ({e}); no se guardará encima");
                return empty(false);
            }
        };
        match serde_json::from_slice::<VoicesFile>(&bytes) {
            Ok(f) if f.space == PRINT_SPACE => VoiceStore {
                voices: f.voices,
                path: path.to_path_buf(),
                writable: true,
            },
            Ok(f) => {
                set_aside(path, &bytes, &f.space);
                empty(true)
            }
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
                empty(true)
            }
        }
    }

    pub fn save(&self) -> io::Result<()> {
        if !self.writable {
            return Err(io::Error::other(
                "las voces guardadas no se pudieron leer; no se guarda encima para no perderlas",
            ));
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = VoicesFile {
            version: 1,
            space: current_space(),
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

/// Aparta `voces.json` de otro espacio de huellas (`bytes`, su contenido) a
/// `voces.json.<espacio>.bak`: con este modelo ya no reconocen a nadie.
fn set_aside(path: &Path, bytes: &[u8], space: &str) {
    // el espacio viene del archivo: solo letras, números y guiones en el nombre de la copia
    let tag: String = space
        .chars()
        .take(40)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let bak = path.with_extension(format!("json.{tag}.bak"));
    match fs::write(&bak, bytes).and_then(|()| fs::remove_file(path)) {
        Ok(()) => log::warn!(
            "Voces guardadas con otro modelo ({tag}): no se usan; apartadas en {}",
            bak.display()
        ),
        Err(e) => log::warn!("Voces guardadas con otro modelo ({tag}): no se usan ({e})"),
    }
}

/// Borra las copias de `voces.json` (también tienen huellas): la de un archivo dañado
/// (`voces.json.bad`) y las de otro espacio (`voces.json.<espacio>.bak`) más antiguas que
/// `older_than`, y un `voces.json.tmp` de un guardado a medias. Devuelve cuántas.
pub fn prune_voice_backups(voices: &Path, older_than: Duration) -> usize {
    let (Some(dir), Some(name)) = (voices.parent(), voices.file_name().and_then(|n| n.to_str()))
    else {
        return 0;
    };
    let prefix = format!("{name}.");
    remove_old(dir, |file| match file.strip_prefix(&prefix)? {
        "tmp" => Some(STALE_TMP),
        "bad" => Some(older_than),
        rest if rest.ends_with(".bak") => Some(older_than),
        _ => None,
    })
}

/// Borra los archivos de `dir` sin tocar desde hace más de lo que `max_age` diga de su nombre
/// (None: no se borra). Devuelve cuántos.
pub(crate) fn remove_old(dir: &Path, max_age: impl Fn(&str) -> Option<Duration>) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            let Some(age) = e.file_name().to_str().and_then(&max_age) else {
                return false;
            };
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > age)
        })
        .filter(|e| fs::remove_file(e.path()).is_ok())
        .count()
}

pub(crate) fn mix(a: &[f32], a_s: f64, b: &[f32], b_s: f64) -> Vec<f32> {
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

/// La voz a la que vota más de la mitad de los segmentos que votan (al menos `MIN_VOTERS`, sin
/// empate con otra voz): (índice, proporción, segundos).
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
    if voters.len() < MIN_VOTERS {
        return None;
    }
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
    let tied = counts.iter().filter(|&&c| c == n).count() > 1;
    if n == 0 || tied || 2 * n <= voters.len() {
        return None;
    }
    let secs = cluster.iter().map(|(_, s)| s).sum();
    Some((who, n as f64 / voters.len() as f64, secs))
}

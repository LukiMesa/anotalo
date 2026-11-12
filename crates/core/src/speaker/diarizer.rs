//! Las voces de "Otros" en una llamada (spec §2.4.5). En vivo, cada segmento recibe la etiqueta de
//! su grupo ("Otros" mientras solo hay una voz; "Persona N" si hay más). Al colgar se agrupa la
//! llamada entera, se buscan las voces guardadas y salen las etiquetas finales y las huellas de
//! cada persona de la llamada (para poder nombrarla después).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::cluster::{Online, end_of_call, weighted_centroid};
use super::memory::{StoredVoice, name_clusters};

/// "Persona 1", "Persona 2"…
pub const PERSON: &str = "Persona";

/// Una persona de una llamada: su etiqueta final y su huella.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CallVoice {
    pub label: String,
    /// Huella centrada y de norma 1 (media de sus segmentos, ponderada por duración).
    pub print: Vec<f32>,
    pub seconds: f64,
    /// El usuario le puso este nombre (con `nombrar`); si lo corrige, se corrige la voz guardada.
    #[serde(default)]
    pub named: bool,
}

pub struct Finished {
    /// Etiqueta final de cada segmento (por su id).
    pub labels: HashMap<u32, String>,
    pub voices: Vec<CallVoice>,
}

struct Segment {
    dur: f64,
    emb: Option<Vec<f32>>,
}

pub struct Diarizer {
    segments: Vec<Segment>,
    online: Online,
    last_live: Option<usize>,
    memory: Vec<StoredVoice>,
    label_others: String,
}

fn person(n: usize) -> String {
    format!("{PERSON} {n}")
}

impl Diarizer {
    pub fn new(memory: Vec<StoredVoice>, label_others: &str) -> Diarizer {
        Diarizer {
            segments: Vec::new(),
            online: Online::new(),
            last_live: None,
            memory,
            label_others: label_others.to_string(),
        }
    }

    /// Un segmento de "Otros" (en orden): su id y la etiqueta que se ve ahora. Sin huella, va con
    /// la voz del segmento anterior que la tuvo.
    pub fn add(&mut self, start_s: f64, end_s: f64, emb: Option<Vec<f32>>) -> (u32, String) {
        let dur = (end_s - start_s).max(0.0);
        let live = match &emb {
            Some(e) => {
                let c = self.online.add(e, dur);
                self.last_live = Some(c);
                c
            }
            None => self.last_live.unwrap_or(0),
        };
        let id = self.segments.len() as u32;
        self.segments.push(Segment { dur, emb });
        let label = if self.online.len() <= 1 {
            self.label_others.clone()
        } else {
            person(live + 1)
        };
        (id, label)
    }

    /// Agrupa la llamada entera y pone los nombres guardados que se reconozcan.
    pub fn finish(&self) -> Finished {
        self.finish_where(|_| true)
    }

    /// Como `finish`, contando solo los segmentos que dejaron una línea en la transcripción: la
    /// música en espera o el ruido sin texto no son una persona más.
    pub fn finish_with_lines(&self, ids: &HashSet<u32>) -> Finished {
        self.finish_where(|i| ids.contains(&i))
    }

    fn finish_where(&self, keep: impl Fn(u32) -> bool) -> Finished {
        let with: Vec<usize> = (0..self.segments.len())
            .filter(|&i| self.segments[i].emb.is_some() && keep(i as u32))
            .collect();
        if with.is_empty() {
            return Finished {
                labels: (0..self.segments.len() as u32)
                    .map(|i| (i, self.label_others.clone()))
                    .collect(),
                voices: Vec::new(),
            };
        }
        let embs: Vec<Vec<f32>> = with
            .iter()
            .map(|&i| self.segments[i].emb.clone().unwrap_or_default())
            .collect();
        let durs: Vec<f64> = with.iter().map(|&i| self.segments[i].dur).collect();
        let cluster_of = end_of_call(&embs, &durs);
        let k = cluster_of.iter().copied().max().map_or(0, |m| m + 1);
        let members: Vec<Vec<usize>> = (0..k)
            .map(|c| (0..with.len()).filter(|&j| cluster_of[j] == c).collect())
            .collect();
        let for_vote: Vec<Vec<(Vec<f32>, f64)>> = members
            .iter()
            .map(|m| m.iter().map(|&j| (embs[j].clone(), durs[j])).collect())
            .collect();
        let names = name_clusters(&for_vote, &self.memory);
        let mut unnamed = 0;
        let cluster_labels: Vec<String> = names
            .iter()
            .map(|n| match n {
                Some(name) => name.clone(),
                None if k == 1 => self.label_others.clone(),
                None => {
                    unnamed += 1;
                    person(unnamed)
                }
            })
            .collect();

        // sin huella: la etiqueta del segmento anterior que la tuvo (la del primer grupo al principio)
        let mut labels = HashMap::new();
        let mut current = cluster_labels[0].clone();
        let mut next_with = 0;
        for i in 0..self.segments.len() {
            if with.get(next_with) == Some(&i) {
                current = cluster_labels[cluster_of[next_with]].clone();
                next_with += 1;
            }
            labels.insert(i as u32, current.clone());
        }
        let voices: Vec<CallVoice> = members
            .iter()
            .zip(&cluster_labels)
            .map(|(m, label)| CallVoice {
                label: label.clone(),
                print: weighted_centroid(&embs, &durs, m),
                seconds: m.iter().map(|&j| durs[j]).sum(),
                named: false,
            })
            .collect();
        log::info!(
            "Voces de la llamada: {} ({} segmentos)",
            cluster_labels.join(", "),
            self.segments.len()
        );
        Finished { labels, voices }
    }
}

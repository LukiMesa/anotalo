//! Mide la separación de voces sobre las llamadas simuladas del spike (spec §2.4.5 y §7), con el
//! mismo camino que la app: TitaNet en tract, agrupación en vivo y al colgar, memoria de voces.
//!
//! Uso: cargo run --release -p anotalo --example diarize-eval -- <calls_dir> [--version deg|clean] [--threads N]
//! `<calls_dir>` tiene `segments_<v>.json`, `<v>/<id>.wav`, `meta/<id>.json` y `speakers.json`.
//! Sale con 0 si cumple la puerta de la spec: al colgar ≥ 90 % de aciertos en segmentos ≥ 2 s y
//! como mucho un 5 % de personas desconocidas con nombre.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anotalo_core::models::{HttpFetch, ensure_titanet, models_dir};
use anotalo_core::speaker::cluster::Online;
use anotalo_core::speaker::diarizer::Diarizer;
use anotalo_core::speaker::embed::{Embed, Embedder, l2n};
use anotalo_core::speaker::memory::StoredVoice;
use serde_json::Value;

struct Seg {
    dur: f64,
    start: f64,
    gt: Option<String>,
    emb: Option<Vec<f32>>,
}

struct Call {
    id: String,
    n: usize,
    segs: Vec<Seg>,
}

fn read_wav(path: &Path) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    match r.spec().sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(Result::unwrap).collect(),
        hound::SampleFormat::Int => r
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.0)
            .collect(),
    }
}

/// El hablante con más solapamiento con [start, end), como `gt_for` del spike.
fn gt_for(turns: &[Value], start: f64, end: f64) -> Option<String> {
    let mut ov: BTreeMap<String, f64> = BTreeMap::new();
    for t in turns {
        let (ts, te) = (t["start"].as_f64().unwrap(), t["end"].as_f64().unwrap());
        let o = end.min(te) - start.max(ts);
        if o > 0.0 {
            *ov.entry(t["spk"].as_str().unwrap().to_string())
                .or_default() += o;
        }
    }
    ov.into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(s, _)| s)
}

fn load_call(dir: &Path, version: &str, id: &str, segs: &[Value], emb: &Embedder) -> (Call, f64) {
    let meta: Value = serde_json::from_slice(
        &std::fs::read(dir.join("meta").join(format!("{id}.json"))).unwrap(),
    )
    .unwrap();
    let turns = meta["turns"].as_array().unwrap();
    let audio = read_wav(&dir.join(version).join(format!("{id}.wav")));
    let mut secs = 0.0;
    let segs = segs
        .iter()
        .map(|s| {
            let (start, end) = (s[0].as_f64().unwrap(), s[1].as_f64().unwrap());
            let (st, n) = (
                s[2].as_u64().unwrap() as usize,
                s[3].as_u64().unwrap() as usize,
            );
            let t = Instant::now();
            let e = emb.embed(&audio[st..(st + n).min(audio.len())]);
            secs += t.elapsed().as_secs_f64();
            Seg {
                dur: end - start,
                start,
                gt: gt_for(turns, start, end),
                emb: e,
            }
        })
        .collect();
    let call = Call {
        id: id.to_string(),
        n: meta["n"].as_u64().unwrap() as usize,
        segs,
    };
    (call, secs)
}

/// Aciertos con la mejor correspondencia uno a uno etiqueta → hablante (fuerza bruta hasta 9
/// etiquetas; si hay más, voraz).
fn mapped_correct(pred: &[String], gt: &[Option<String>]) -> Vec<bool> {
    let labels: Vec<&String> = pred.iter().collect::<HashSet<_>>().into_iter().collect();
    let speakers: Vec<&String> = gt
        .iter()
        .flatten()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut m = vec![vec![0usize; speakers.len()]; labels.len()];
    for (p, g) in pred.iter().zip(gt) {
        if let Some(g) = g {
            let i = labels.iter().position(|l| *l == p).unwrap();
            let j = speakers.iter().position(|s| *s == g).unwrap();
            m[i][j] += 1;
        }
    }
    let assign: Vec<Option<usize>> = if labels.len() <= 9 {
        fn best(
            i: usize,
            m: &[Vec<usize>],
            used: &mut Vec<bool>,
            cur: &mut Vec<Option<usize>>,
        ) -> (usize, Vec<Option<usize>>) {
            if i == m.len() {
                return (0, cur.clone());
            }
            let mut top = (0, Vec::new());
            cur.push(None);
            let (s, a) = best(i + 1, m, used, cur);
            if s >= top.0 || top.1.is_empty() {
                top = (s, a);
            }
            cur.pop();
            for j in 0..used.len() {
                if !used[j] {
                    used[j] = true;
                    cur.push(Some(j));
                    let (s, a) = best(i + 1, m, used, cur);
                    if s + m[i][j] > top.0 {
                        top = (s + m[i][j], a);
                    }
                    cur.pop();
                    used[j] = false;
                }
            }
            top
        }
        best(0, &m, &mut vec![false; speakers.len()], &mut Vec::new()).1
    } else {
        let mut a = vec![None; labels.len()];
        let mut used = vec![false; speakers.len()];
        let mut cells: Vec<(usize, usize, usize)> = (0..labels.len())
            .flat_map(|i| (0..speakers.len()).map(move |j| (i, j, 0)))
            .map(|(i, j, _)| (i, j, m[i][j]))
            .collect();
        cells.sort_by_key(|c| std::cmp::Reverse(c.2));
        for (i, j, _) in cells {
            if a[i].is_none() && !used[j] {
                a[i] = Some(j);
                used[j] = true;
            }
        }
        a
    };
    pred.iter()
        .zip(gt)
        .map(|(p, g)| {
            let i = labels.iter().position(|l| *l == p).unwrap();
            g.as_ref()
                .is_some_and(|g| assign[i].is_some_and(|j| speakers[j] == g))
        })
        .collect()
}

#[derive(Default)]
struct Acc {
    ok: usize,
    n: usize,
    ok2: usize,
    n2: usize,
}

impl Acc {
    fn add(&mut self, call: &Call, pred: &[String]) {
        let gt: Vec<Option<String>> = call.segs.iter().map(|s| s.gt.clone()).collect();
        for (s, ok) in call.segs.iter().zip(mapped_correct(pred, &gt)) {
            if s.gt.is_some() {
                self.n += 1;
                self.ok += usize::from(ok);
                if s.dur >= 2.0 {
                    self.n2 += 1;
                    self.ok2 += usize::from(ok);
                }
            }
        }
    }
    fn line(&self) -> String {
        format!(
            "{:.3} (todos) / {:.3} (≥ 2 s)",
            self.ok as f64 / self.n.max(1) as f64,
            self.ok2 as f64 / self.n2.max(1) as f64
        )
    }
}

/// Etiquetas en vivo: el grupo de `Online` (los segmentos sin huella siguen al anterior).
fn live_labels(call: &Call) -> Vec<String> {
    let mut o = Online::new();
    let mut last = 0;
    call.segs
        .iter()
        .map(|s| {
            if let Some(e) = &s.emb {
                last = o.add(e, s.dur);
            }
            last.to_string()
        })
        .collect()
}

fn final_labels(call: &Call, memory: Vec<StoredVoice>) -> Vec<String> {
    let mut d = Diarizer::new(memory, "Otros");
    for s in &call.segs {
        d.add(s.start, s.start + s.dur, s.emb.clone());
    }
    let f = d.finish();
    (0..call.segs.len() as u32)
        .map(|i| f.labels[&i].clone())
        .collect()
}

fn is_tune(id: &str) -> bool {
    id.split('_')
        .nth(1)
        .and_then(|k| k.parse::<u32>().ok())
        .is_some_and(|k| k < 8)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first().map(PathBuf::from) else {
        eprintln!("Uso: diarize-eval <calls_dir> [--version deg|clean] [--threads N]");
        return ExitCode::from(2);
    };
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let version = opt("--version").unwrap_or_else(|| "deg".into());
    let threads = opt("--threads")
        .and_then(|t| t.parse().ok())
        .or_else(anotalo_whisper::physical_cores)
        .unwrap_or(4)
        .max(1);

    let model = ensure_titanet(&models_dir(), &HttpFetch::default(), &mut |_| {}).unwrap();
    let emb = Embedder::load(&model).unwrap();
    let segments: BTreeMap<String, Vec<Value>> = serde_json::from_slice(
        &std::fs::read(dir.join(format!("segments_{version}.json"))).unwrap(),
    )
    .unwrap();
    let known: HashSet<String> =
        serde_json::from_slice::<Value>(&std::fs::read(dir.join("speakers.json")).unwrap())
            .unwrap()["known"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();

    let t = Instant::now();
    let ids: Vec<&String> = segments.keys().collect();
    let mut calls: Vec<(Call, f64)> = std::thread::scope(|sc| {
        let chunks: Vec<Vec<&String>> = (0..threads)
            .map(|i| ids.iter().skip(i).step_by(threads).copied().collect())
            .collect();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let (dir, version, segments, emb) = (&dir, &version, &segments, &emb);
                sc.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|id| load_call(dir, version, id, &segments[id], emb))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    calls.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    let embed_secs: f64 = calls.iter().map(|(_, s)| s).sum();
    let calls: Vec<Call> = calls.into_iter().map(|(c, _)| c).collect();
    let n_segs: usize = calls.iter().map(|c| c.segs.len()).sum();
    let none = calls
        .iter()
        .flat_map(|c| &c.segs)
        .filter(|s| s.emb.is_none())
        .count();
    println!(
        "diarize-eval {version}: {} llamadas, {n_segs} segmentos ({none} sin huella), {:.0} ms por segmento, {:.0} s en total",
        calls.len(),
        1000.0 * embed_secs / n_segs.max(1) as f64,
        t.elapsed().as_secs_f64()
    );

    let (mut live, mut end) = (Acc::default(), Acc::default());
    let mut count_err = 0.0;
    for c in &calls {
        live.add(c, &live_labels(c));
        let f = final_labels(c, Vec::new());
        count_err += (f.iter().collect::<HashSet<_>>().len() as f64 - c.n as f64).abs();
        end.add(c, &f);
    }
    println!("En vivo:   {}", live.line());
    println!(
        "Al colgar: {}; personas, error medio {:.2}",
        end.line(),
        count_err / calls.len().max(1) as f64
    );

    // Memoria de voces, dos mitades: se inscribe en una y se prueba en la otra.
    let (mut k_ok, mut k_wrong, mut k_n, mut u_fa, mut u_n) = (0, 0, 0, 0, 0);
    for enroll_tune in [true, false] {
        let mut memory: Vec<StoredVoice> = Vec::new();
        for c in calls.iter().filter(|c| is_tune(&c.id) == enroll_tune) {
            let speakers: HashSet<&String> = c.segs.iter().filter_map(|s| s.gt.as_ref()).collect();
            for s in speakers {
                if !known.contains(s) || memory.iter().any(|v| &v.name == s) {
                    continue;
                }
                let e: Vec<&Vec<f32>> = c
                    .segs
                    .iter()
                    .filter(|x| x.gt.as_ref() == Some(s) && x.dur >= 1.0)
                    .filter_map(|x| x.emb.as_ref())
                    .collect();
                if e.is_empty() {
                    continue;
                }
                let mut sum = vec![0f32; e[0].len()];
                for v in &e {
                    for (a, b) in sum.iter_mut().zip(v.iter()) {
                        *a += b;
                    }
                }
                memory.push(StoredVoice {
                    name: s.clone(),
                    print: l2n(&sum),
                    seconds: 60.0,
                });
            }
        }
        let names: HashSet<String> = memory.iter().map(|v| v.name.clone()).collect();
        for c in calls.iter().filter(|c| is_tune(&c.id) != enroll_tune) {
            let labels = final_labels(c, memory.clone());
            let mut clusters: HashMap<&String, Vec<&Option<String>>> = HashMap::new();
            for (l, s) in labels.iter().zip(&c.segs) {
                clusters.entry(l).or_default().push(&s.gt);
            }
            for (label, gts) in clusters {
                let mut votes: HashMap<&String, usize> = HashMap::new();
                for g in gts.iter().copied().flatten() {
                    *votes.entry(g).or_default() += 1;
                }
                let Some((truth, _)) = votes.into_iter().max_by_key(|(_, n)| *n) else {
                    continue;
                };
                let named = names.contains(label).then_some(label);
                if names.contains(truth) {
                    k_n += 1;
                    k_ok += usize::from(named == Some(truth));
                    k_wrong += usize::from(named.is_some_and(|n| n != truth));
                } else {
                    u_n += 1;
                    u_fa += usize::from(named.is_some());
                }
            }
        }
    }
    let rate = |a: usize, b: usize| a as f64 / b.max(1) as f64;
    println!(
        "Memoria de voces: conocidos con su nombre {:.3}, con otro nombre {:.3}, desconocidos con nombre {:.3} ({k_n} conocidos, {u_n} desconocidos)",
        rate(k_ok, k_n),
        rate(k_wrong, k_n),
        rate(u_fa, u_n)
    );
    let end2 = rate(end.ok2, end.n2);
    let fa = rate(u_fa, u_n);
    let pass = end2 >= 0.90 && fa <= 0.05;
    println!(
        "Puerta (spec §2.4.5): al colgar {end2:.3} ≥ 0,90 y desconocidos con nombre {fa:.3} ≤ 0,05 → {}",
        if pass { "CUMPLE" } else { "NO CUMPLE" }
    );
    if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

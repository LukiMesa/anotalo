//! Agrupar las huellas de "Otros" por persona (spec §2.4.5, parámetros del spike con huellas
//! centradas y audio de llamada): en vivo, al centroide más cercano; al colgar, agrupación
//! jerárquica por enlace medio con distancia coseno. Port de `evaluate.py` del spike.

/// En vivo: un segmento se une al grupo más parecido si el coseno llega a esto.
pub const LIVE_JOIN: f32 = 0.30;
/// En vivo: un segmento más corto nunca abre un grupo nuevo.
pub const LIVE_MIN_NEW_S: f64 = 1.0;
/// Al colgar: se unen grupos mientras su distancia media (1 − coseno) no pase de esto.
pub const END_DIST: f32 = 0.70;
/// Al colgar: solo los segmentos así de largos forman los grupos; el resto va al más cercano.
pub const END_MIN_S: f64 = 1.0;
/// Al colgar: un grupo con menos voz que esto se reparte entre los demás.
pub const MIN_CLUSTER_S: f64 = 10.0;

fn dot(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum()
}

fn l2n64(v: &[f64]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f64>().sqrt() + 1e-12;
    v.iter().map(|x| (x / n) as f32).collect()
}

/// Grupos en vivo: centroides como sumas, comparados ya normalizados.
#[derive(Default)]
pub struct Online {
    sums: Vec<Vec<f64>>,
}

impl Online {
    pub fn new() -> Online {
        Online::default()
    }

    pub fn len(&self) -> usize {
        self.sums.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sums.is_empty()
    }

    /// El grupo del segmento (0, 1, 2… por orden de creación).
    pub fn add(&mut self, e: &[f32], dur_s: f64) -> usize {
        if !self.sums.is_empty() {
            let (j, s) = self
                .sums
                .iter()
                .map(|c| dot(&l2n64(c), e))
                .enumerate()
                .fold(
                    (0, f64::MIN),
                    |best, (i, s)| if s > best.1 { (i, s) } else { best },
                );
            if s >= f64::from(LIVE_JOIN) || dur_s < LIVE_MIN_NEW_S {
                for (c, x) in self.sums[j].iter_mut().zip(e) {
                    *c += f64::from(*x);
                }
                return j;
            }
        }
        self.sums.push(e.iter().map(|x| f64::from(*x)).collect());
        self.sums.len() - 1
    }
}

/// `l2n(Σ dur·e)` de los segmentos `members`.
pub fn weighted_centroid(embs: &[Vec<f32>], durs: &[f64], members: &[usize]) -> Vec<f32> {
    let dim = embs.first().map_or(0, Vec::len);
    let mut s = vec![0f64; dim];
    for &i in members {
        for (a, x) in s.iter_mut().zip(&embs[i]) {
            *a += durs[i] * f64::from(*x);
        }
    }
    l2n64(&s)
}

/// El índice del centroide más parecido (el primero si empatan).
fn nearest(e: &[f32], centroids: &[Vec<f32>]) -> usize {
    centroids
        .iter()
        .map(|c| dot(c, e))
        .enumerate()
        .fold(
            (0, f64::MIN),
            |best, (i, s)| if s > best.1 { (i, s) } else { best },
        )
        .0
}

/// Agrupación al colgar. Devuelve una etiqueta por segmento, numeradas por orden de aparición.
pub fn end_of_call(embs: &[Vec<f32>], durs: &[f64]) -> Vec<usize> {
    let n = embs.len();
    if n == 0 {
        return Vec::new();
    }
    let mut big: Vec<usize> = (0..n).filter(|&i| durs[i] >= END_MIN_S).collect();
    if big.len() < 2 {
        let mut by_len: Vec<usize> = (0..n).collect();
        by_len.sort_by(|&a, &b| durs[b].total_cmp(&durs[a]));
        by_len.truncate(n.min(2));
        big = by_len;
    }
    let big_labels = average_linkage(&big.iter().map(|&i| &embs[i][..]).collect::<Vec<_>>());
    let k = big_labels.iter().copied().max().map_or(0, |m| m + 1);
    let centroids: Vec<Vec<f32>> = (0..k)
        .map(|c| {
            let members: Vec<usize> = big
                .iter()
                .zip(&big_labels)
                .filter(|(_, l)| **l == c)
                .map(|(i, _)| *i)
                .collect();
            weighted_centroid(embs, durs, &members)
        })
        .collect();
    let mut pred: Vec<usize> = embs.iter().map(|e| nearest(e, &centroids)).collect();
    for (&i, &l) in big.iter().zip(&big_labels) {
        pred[i] = l;
    }
    merge_small_clusters(embs, durs, &mut pred);
    first_appearance(&pred)
}

/// Los segmentos de los grupos con menos de `MIN_CLUSTER_S` de voz van al grupo grande más
/// cercano, si hay alguno grande y no lo son todos.
fn merge_small_clusters(embs: &[Vec<f32>], durs: &[f64], pred: &mut [usize]) {
    let mut ks: Vec<usize> = pred.to_vec();
    ks.sort_unstable();
    ks.dedup();
    let members = |k: usize, pred: &[usize]| -> Vec<usize> {
        (0..pred.len()).filter(|&i| pred[i] == k).collect()
    };
    let big: Vec<usize> = ks
        .iter()
        .copied()
        .filter(|&k| members(k, pred).iter().map(|&i| durs[i]).sum::<f64>() >= MIN_CLUSTER_S)
        .collect();
    if big.is_empty() || big.len() == ks.len() {
        return;
    }
    let centroids: Vec<Vec<f32>> = big
        .iter()
        .map(|&k| weighted_centroid(embs, durs, &members(k, pred)))
        .collect();
    for i in 0..pred.len() {
        if !big.contains(&pred[i]) {
            pred[i] = big[nearest(&embs[i], &centroids)];
        }
    }
}

fn first_appearance(pred: &[usize]) -> Vec<usize> {
    let mut map: Vec<(usize, usize)> = Vec::new();
    pred.iter()
        .map(|&p| match map.iter().find(|(from, _)| *from == p) {
            Some((_, to)) => *to,
            None => {
                map.push((p, map.len()));
                map.len() - 1
            }
        })
        .collect()
}

/// Enlace medio con distancia 1 − coseno (Lance–Williams), uniendo mientras la distancia más
/// pequeña no pase de `END_DIST` (como `fcluster(..., t, criterion="distance")` de scipy).
fn average_linkage(embs: &[&[f32]]) -> Vec<usize> {
    let n = embs.len();
    if n <= 1 {
        return vec![0; n];
    }
    let mut d = vec![0f64; n * n];
    for i in 0..n {
        for j in i + 1..n {
            let v = 1.0 - dot(embs[i], embs[j]);
            d[i * n + j] = v;
            d[j * n + i] = v;
        }
    }
    let mut size = vec![1usize; n];
    let mut active = vec![true; n];
    let mut owner: Vec<usize> = (0..n).collect();
    let limit = f64::from(END_DIST);
    loop {
        let mut best: Option<(usize, usize, f64)> = None;
        for i in 0..n {
            if !active[i] {
                continue;
            }
            for j in i + 1..n {
                if active[j] && best.is_none_or(|(_, _, b)| d[i * n + j] < b) {
                    best = Some((i, j, d[i * n + j]));
                }
            }
        }
        let Some((a, b, dist)) = best else { break };
        if dist > limit {
            break;
        }
        // b se une a a
        for k in 0..n {
            if active[k] && k != a && k != b {
                let v = (size[a] as f64 * d[a * n + k] + size[b] as f64 * d[b * n + k])
                    / (size[a] + size[b]) as f64;
                d[a * n + k] = v;
                d[k * n + a] = v;
            }
        }
        size[a] += size[b];
        active[b] = false;
        for o in owner.iter_mut() {
            if *o == b {
                *o = a;
            }
        }
    }
    first_appearance(&owner)
}

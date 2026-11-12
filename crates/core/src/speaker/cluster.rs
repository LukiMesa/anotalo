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

/// Posición de d(i, j), con i < j, en la mitad de arriba de la matriz guardada fila a fila.
fn upper(n: usize, i: usize, j: usize) -> usize {
    i * (2 * n - i - 1) / 2 + j - i - 1
}

/// Cuántas columnas de la matriz de distancias se calculan a la vez.
const LANES: usize = 16;

/// La mitad de arriba de la matriz de distancias 1 − coseno, fila a fila. Cada producto
/// escalar suma en el mismo orden que `dot` (da lo mismo, bit a bit), pero por bloques de
/// `LANES` columnas: sus huellas, traspuestas (una fila de `LANES` por dimensión), caben en la
/// caché y las `LANES` sumas de cada fila avanzan a la vez.
fn upper_distances(embs: &[&[f32]]) -> Vec<f64> {
    let n = embs.len();
    let dim = embs.first().map_or(0, |e| e.len());
    let mut d = vec![0f64; n * n.saturating_sub(1) / 2];
    let mut block = vec![[0f64; LANES]; dim];
    for j0 in (1..n).step_by(LANES) {
        let j1 = (j0 + LANES).min(n);
        block.fill([0.0; LANES]);
        for (l, e) in embs[j0..j1].iter().enumerate() {
            for (row, x) in block.iter_mut().zip(*e) {
                row[l] = f64::from(*x);
            }
        }
        // las filas con alguna columna j > i en el bloque
        for (i, e) in embs[..j1 - 1].iter().enumerate() {
            let mut s = [0f64; LANES];
            for (x, ys) in e.iter().zip(&block) {
                let x = f64::from(*x);
                for (s, y) in s.iter_mut().zip(ys) {
                    *s += x * y;
                }
            }
            let from = j0.max(i + 1);
            let at = upper(n, i, from);
            for (o, s) in d[at..at + j1 - from].iter_mut().zip(&s[from - j0..]) {
                *o = 1.0 - s;
            }
        }
    }
    d
}

/// Enlace medio con distancia 1 − coseno (Lance–Williams), uniendo mientras la distancia más
/// pequeña no pase de `END_DIST` (como `fcluster(..., t, criterion="distance")` de scipy).
///
/// Cadena de vecinos más cercanos, como `linkage` de scipy: el enlace medio es reducible (el
/// grupo unido nunca queda más cerca de un tercero que el más cercano de los dos), así que da el
/// mismo árbol que unir siempre el par más cercano, pero en O(n²) y no en O(n³). Un grupo cuyo
/// vecino más cercano ya pasa de `END_DIST` no se unirá a nada: se cierra. Solo se guarda la
/// mitad de arriba de la matriz de distancias.
fn average_linkage(embs: &[&[f32]]) -> Vec<usize> {
    let n = embs.len();
    if n <= 1 {
        return vec![0; n];
    }
    let at = |i: usize, j: usize| {
        if i < j {
            upper(n, i, j)
        } else {
            upper(n, j, i)
        }
    };
    let mut d = upper_distances(embs);
    let mut size = vec![1usize; n];
    // los grupos que aún pueden unirse a otro
    let mut open = vec![true; n];
    let mut left = n;
    let mut owner: Vec<usize> = (0..n).collect();
    let mut chain: Vec<usize> = Vec::with_capacity(n);
    let limit = f64::from(END_DIST);
    while left > 1 {
        if chain.is_empty() {
            chain.extend(open.iter().position(|&o| o));
        }
        let x = chain[chain.len() - 1];
        // el vecino más cercano de x; en un empate gana el anterior de la cadena (si no, la
        // cadena podría dar vueltas) y después el primer índice
        let prev = chain.len().checked_sub(2).map(|p| chain[p]);
        let (mut y, mut best) = prev.map_or((x, f64::INFINITY), |p| (p, d[at(x, p)]));
        for i in (0..n).filter(|&i| open[i] && i != x) {
            let v = d[at(x, i)];
            if v < best {
                (y, best) = (i, v);
            }
        }
        if best > limit {
            // nada queda a `END_DIST` o menos de x, y unir otros grupos no los acerca: x ya no
            // cambia
            open[x] = false;
            left -= 1;
            chain.pop();
            continue;
        }
        if Some(y) != prev {
            chain.push(y);
            continue;
        }
        // x e y son vecinos mutuos: el mayor se une al menor
        chain.truncate(chain.len() - 2);
        let (a, b) = (x.min(y), x.max(y));
        for k in (0..n).filter(|&k| open[k] && k != a && k != b) {
            let (da, db) = (d[at(a, k)], d[at(b, k)]);
            let v = (size[a] as f64 * da + size[b] as f64 * db) / (size[a] + size[b]) as f64;
            // en coma flotante la media puede quedar un ulp por debajo de la menor de las dos:
            // acotada, el enlace sigue siendo reducible y la cadena nunca vuelve atrás
            d[at(a, k)] = v.max(da.min(db));
        }
        size[a] += size[b];
        open[b] = false;
        left -= 1;
        for o in owner.iter_mut().filter(|o| **o == b) {
            *o = a;
        }
    }
    first_appearance(&owner)
}

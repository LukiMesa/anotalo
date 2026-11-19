//! Líneas tuyas que repiten lo que otra persona dijo a la vez. Sin auriculares el micrófono oye a
//! los demás por los altavoces y lo que dicen vuelve a salir como tuyo (eco). Se comparan las
//! palabras (sin tildes, mayúsculas ni puntuación, con margen para las que Whisper oye un poco
//! distintas) con lo que dijeron los demás alrededor de ese momento: los tramos repetidos se quitan
//! y lo demás (tu respuesta, pegada al eco en la misma línea) se queda.

use crate::library::fold;

/// Cuánto antes y después de tu línea se mira lo que dijeron los demás.
pub const WINDOW_S: f64 = 3.0;
/// Un tramo de eco tiene al menos estas palabras repetidas...
const MIN_SPAN_WORDS: usize = 4;
/// ...con, entre una y otra, como mucho estas palabras distintas seguidas en tu línea...
const MAX_GAP_MINE: usize = 2;
/// ...y estas en la de los demás (el micrófono no llega a oírlo todo).
const MAX_GAP_OTHERS: usize = 3;
/// Una línea corta es eco entera si al menos esta parte de sus palabras se repite y la otra persona
/// empezó a hablar casi a la vez (`SHORT_START_S`).
const SHORT_SHARE: f64 = 0.75;
const SHORT_START_S: f64 = 1.5;
/// Lo que queda junto a un tramo de eco con menos palabras que esto también se quita: suelen ser
/// palabras del eco mal oídas ("Puede ser" por "pudiera hacer"), no una respuesta.
const MIN_KEPT_WORDS: usize = 3;
/// Una racha (palabras seguidas en las dos líneas) tiene al menos estas palabras.
const MIN_RUN: usize = 2;

/// Una línea: cuándo empezó y terminó (s desde el inicio de la llamada) y qué se dijo.
#[derive(Debug, Clone, Copy)]
pub struct Said<'a> {
    pub start_s: f64,
    pub end_s: f64,
    pub text: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Echo {
    /// Nada de la línea es eco.
    No,
    /// Toda la línea es eco.
    All,
    /// Parte es eco: queda este texto, que empieza (más o menos) en `start_s`.
    Part { start_s: f64, text: String },
}

fn norm(word: &str) -> String {
    fold(word).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Las palabras (tal cual y normalizadas); fuera lo que es solo puntuación.
fn words(text: &str) -> Vec<(&str, String)> {
    text.split_whitespace()
        .map(|w| (w, norm(w)))
        .filter(|(_, n)| !n.is_empty())
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = (diag + usize::from(ca != *cb)).min(up + 1).min(row[j] + 1);
            diag = up;
        }
    }
    row[b.len()]
}

/// La misma palabra, quizá oída un poco distinta ("producto" / "productos").
fn same(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    let short = la.min(lb);
    let max = if short >= 7 { 2 } else { 1 };
    short >= 4 && la.abs_diff(lb) <= max && edit_distance(a, b) <= max
}

/// Rachas de palabras seguidas que están igual de seguidas en lo de los demás: (inicio tuyo,
/// inicio suyo, largo), ordenadas por su inicio en tu línea y sin pisarse. Las más largas se
/// eligen primero: una palabra suelta repetida ("sí", "que") no es eco, una frase sí.
fn runs(mine: &[String], theirs: &[String]) -> Vec<(usize, usize, usize)> {
    let (n, m) = (mine.len(), theirs.len());
    let at = |i: usize, j: usize| i * (m + 1) + j;
    // largo de la racha que empieza en (i, j)
    let mut len = vec![0usize; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            if same(&mine[i], &theirs[j]) {
                len[at(i, j)] = len[at(i + 1, j + 1)] + 1;
            }
        }
    }
    let mut found: Vec<(usize, usize, usize)> = Vec::new();
    for i in 0..n {
        for j in 0..m {
            let l = len[at(i, j)];
            let starts = i == 0 || j == 0 || len[at(i - 1, j - 1)] == 0;
            if l >= MIN_RUN && starts {
                found.push((i, j, l));
            }
        }
    }
    found.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    let mut taken = vec![false; n];
    let mut out = Vec::new();
    for (i, j, l) in found {
        // de la racha, los trozos seguidos que aún no se usaron
        let mut k = i;
        while k < i + l {
            if taken[k] {
                k += 1;
                continue;
            }
            let a = k;
            while k < i + l && !taken[k] {
                k += 1;
            }
            if k - a >= MIN_RUN {
                taken[a..k].iter_mut().for_each(|t| *t = true);
                out.push((a, j + (a - i), k - a));
            }
        }
    }
    out.sort_by_key(|r| r.0);
    out
}

/// Qué parte de `mine` (una línea tuya) repite lo que dijeron los demás (`others`, sus líneas) a
/// la vez.
pub fn echo_in(mine: &Said, others: &[Said]) -> Echo {
    let mut near: Vec<&Said> = others
        .iter()
        .filter(|o| o.start_s <= mine.end_s + WINDOW_S && o.end_s >= mine.start_s - WINDOW_S)
        .collect();
    let my = words(mine.text);
    if my.len() < 2 || near.is_empty() {
        return Echo::No;
    }
    near.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
    let theirs: Vec<String> = near
        .iter()
        .flat_map(|o| words(o.text).into_iter().map(|(_, n)| n))
        .collect();
    let my_norm: Vec<String> = my.iter().map(|(_, n)| n.clone()).collect();

    // tramos: rachas casi seguidas en tu línea y en el mismo orden y casi seguidas en la suya
    // (primera palabra, última, cuántas repetidas)
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    let mut prev_end: Option<(usize, usize)> = None; // fin (exclusivo) de la racha anterior
    for (i, j, l) in runs(&my_norm, &theirs) {
        let joins = prev_end
            .is_some_and(|(pi, pj)| i - pi <= MAX_GAP_MINE && j >= pj && j - pj <= MAX_GAP_OTHERS);
        match spans.last_mut() {
            Some(s) if joins => {
                s.1 = i + l - 1;
                s.2 += l;
            }
            _ => spans.push((i, i + l - 1, l)),
        }
        prev_end = Some((i + l, j + l));
    }
    let n = my.len();
    let together = near
        .iter()
        .any(|o| (o.start_s - mine.start_s).abs() <= SHORT_START_S);
    let mut removed = vec![false; n];
    let mut any = false;
    for &(a, b, k) in &spans {
        let short_repeat = k >= 2 && k as f64 >= SHORT_SHARE * n as f64 && together;
        if k >= MIN_SPAN_WORDS || short_repeat {
            removed[a..=b].iter_mut().for_each(|r| *r = true);
            any = true;
        }
    }
    if !any {
        return Echo::No;
    }
    // lo que queda: tramos seguidos de palabras no repetidas, los de MIN_KEPT_WORDS o más
    let mut kept: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < n {
        if removed[k] {
            k += 1;
            continue;
        }
        let a = k;
        while k < n && !removed[k] {
            k += 1;
        }
        if k - a >= MIN_KEPT_WORDS {
            kept.push((a, k));
        }
    }
    let Some(&(first, _)) = kept.first() else {
        return Echo::All;
    };
    let text = kept
        .iter()
        .flat_map(|&(a, b)| my[a..b].iter().map(|(w, _)| *w))
        .collect::<Vec<_>>()
        .join(" ");
    let start_s = mine.start_s + (mine.end_s - mine.start_s).max(0.0) * first as f64 / n as f64;
    Echo::Part { start_s, text }
}

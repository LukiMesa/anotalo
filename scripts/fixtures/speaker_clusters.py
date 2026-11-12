# Requires the diarization spike's stack: numpy, scipy 1.18.1 and the spike's embeddings (data/emb).
"""Clustering oracle for the Rust speaker crate (M2b): real TitaNet-small embeddings of two degraded
test calls with the spike's online and end-of-call labels (evaluate.py, centered, deg params).

usage: python -I make_cluster_fixtures.py <emb_deg.npz> <mu.bin> <out_dir> <call_id>...

Per call (little-endian): <id>.emb.bin f32 [n,192] raw embeddings; <id>.dur.bin f32 [n];
<id>.online.bin u32 [n]; <id>.ahc.bin u32 [n]. Labels are renumbered by first appearance.
"""
import json, os, sys
import numpy as np
from scipy.cluster.hierarchy import linkage, fcluster

npz, mu_p, out = sys.argv[1:4]
calls = sys.argv[4:]
z = np.load(npz)
mu = np.fromfile(mu_p, dtype=np.float32).astype(np.float64)


def l2n(x):
    return x / (np.linalg.norm(x, axis=-1, keepdims=True) + 1e-12)


def online(E, dur, thr, min_new):
    cents, preds = [], []
    for e, d in zip(E, dur):
        if cents:
            C = l2n(np.array(cents)); s = C @ e; j = int(np.argmax(s))
            if s[j] >= thr or d < min_new:
                cents[j] = cents[j] + e; preds.append(j); continue
        cents.append(e.copy()); preds.append(len(cents) - 1)
    return np.array(preds)


def _ahc(E, dur, thr, min_dur):
    big = np.where(dur >= min_dur)[0]
    if len(big) < 2:
        big = np.argsort(-dur)[:max(1, min(2, len(dur)))]
    if len(big) == 1:
        lab_big = np.array([1])
    else:
        lab_big = fcluster(linkage(E[big], method="average", metric="cosine"), t=thr, criterion="distance")
    labs = np.unique(lab_big)
    C = l2n(np.array([(E[big][lab_big == l] * dur[big][lab_big == l, None]).sum(0) for l in labs]))
    pred = np.argmax(E @ C.T, axis=1)
    pred[big] = np.searchsorted(labs, lab_big)
    return pred


def ahc(E, dur, thr, min_dur, min_cluster_s):
    pred = _ahc(E, dur, thr, min_dur)
    ks = np.unique(pred)
    tot = np.array([dur[pred == k].sum() for k in ks])
    big = ks[tot >= min_cluster_s]
    if 0 < len(big) < len(ks):
        C = l2n(np.array([(E[pred == k] * dur[pred == k, None]).sum(0) for k in big]))
        small = ~np.isin(pred, big)
        pred = pred.copy()
        pred[small] = big[np.argmax(E[small] @ C.T, axis=1)]
    return pred


def first_seen(pred):
    m = {}
    return np.array([m.setdefault(p, len(m)) for p in pred], dtype=np.uint32)


os.makedirs(out, exist_ok=True)
meta = {}
for cid in calls:
    ix = np.where(z["call"] == cid)[0]
    raw = z["emb"][ix].astype(np.float32)
    dur = (z["end"][ix] - z["start"][ix]).astype(np.float32)
    E = l2n(l2n(raw.astype(np.float64)) - mu)
    d = dur.astype(np.float64)
    on = first_seen(online(E, d, 0.30, 1.0))
    ah = first_seen(ahc(E, d, 0.70, 1.0, 10.0))
    raw.tofile(os.path.join(out, f"{cid}.emb.bin"))
    dur.tofile(os.path.join(out, f"{cid}.dur.bin"))
    on.tofile(os.path.join(out, f"{cid}.online.bin"))
    ah.tofile(os.path.join(out, f"{cid}.ahc.bin"))
    meta[cid] = dict(n=int(len(ix)), online_k=int(on.max() + 1), ahc_k=int(ah.max() + 1))
json.dump(meta, open(os.path.join(out, "clusters.json"), "w"), indent=1)
print(json.dumps(meta, indent=1))

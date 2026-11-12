# Requires the diarization spike's stack: sherpa-onnx 2.0.1, kaldi-native-fbank 1.23.0, numpy, soundfile.
# Outputs go to crates/core/tests/fixtures/speaker/ (fbank, embeddings) and crates/core/assets/ (mu).
"""Fixtures for the Rust speaker crate (M2b), from the spike's reference stack.

usage: python -I make_fixtures.py <jfk.wav> <titanet_small.onnx> <emb_deg.npz> <out_dir>

Writes (all little-endian f32):
  jfk_3s.fbank.bin     kaldi-native-fbank features of jfk.wav[0:48000], sherpa's NeMo options, [T,80]
  jfk_short.fbank.bin  the same for jfk.wav[16000:25600] (0.6 s)
  jfk_full.emb.bin     sherpa-onnx TitaNet-small embedding of all of jfk.wav (192)
  jfk_3s.emb.bin       the same for jfk.wav[0:48000]
  titanet_small_mu.bin background mean: mean of the L2-normalised embeddings of the spike's
                       degraded tune calls (call index < 8), as in evaluate.py (192)
  fixtures.json        shapes and sources
"""
import json, os, sys
import numpy as np, soundfile as sf, kaldi_native_fbank as knf, sherpa_onnx

wav, model, npz, out = sys.argv[1:5]
os.makedirs(out, exist_ok=True)
x, sr = sf.read(wav, dtype="float32")
assert sr == 16000 and x.ndim == 1


def fbank(a):
    o = knf.FbankOptions()
    o.frame_opts.dither = 0
    o.frame_opts.snip_edges = True
    o.frame_opts.remove_dc_offset = False
    o.frame_opts.window_type = "hann"
    o.frame_opts.preemph_coeff = 0.97
    o.mel_opts.num_bins = 80
    o.mel_opts.low_freq = 0.0
    o.mel_opts.high_freq = -400.0
    o.mel_opts.is_librosa = True
    fb = knf.OnlineFbank(o)
    fb.accept_waveform(16000, a.tolist())
    fb.input_finished()
    return np.array([fb.get_frame(i) for i in range(fb.num_frames_ready)], dtype=np.float32)


ext = sherpa_onnx.SpeakerEmbeddingExtractor(
    sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=model, num_threads=1, debug=False, provider="cpu"))


def embed(a):
    s = ext.create_stream()
    s.accept_waveform(sample_rate=16000, waveform=np.ascontiguousarray(a))
    s.input_finished()
    return np.array(ext.compute(s), dtype=np.float32)


meta = {}
for name, a in [("jfk_3s", x[0:48000]), ("jfk_short", x[16000:25600])]:
    f = fbank(a)
    f.tofile(os.path.join(out, f"{name}.fbank.bin"))
    meta[f"{name}.fbank.bin"] = dict(frames=f.shape[0], bins=80, samples=len(a))
for name, a in [("jfk_full", x), ("jfk_3s", x[0:48000])]:
    e = embed(a)
    e.tofile(os.path.join(out, f"{name}.emb.bin"))
    meta[f"{name}.emb.bin"] = dict(dim=len(e), samples=len(a))

z = np.load(npz)
emb = z["emb"].astype(np.float64)
tune = np.array([int(str(c).split("_")[1]) < 8 for c in z["call"]])
l2 = emb / (np.linalg.norm(emb, axis=1, keepdims=True) + 1e-12)
mu = l2[tune].mean(0).astype(np.float32)
mu.tofile(os.path.join(out, "titanet_small_mu.bin"))
meta["titanet_small_mu.bin"] = dict(dim=len(mu), segments=int(tune.sum()), source=os.path.basename(npz))
json.dump(meta, open(os.path.join(out, "fixtures.json"), "w"), indent=1)
print(json.dumps(meta, indent=1))

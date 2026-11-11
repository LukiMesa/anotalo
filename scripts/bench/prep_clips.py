"""Clips en español para el bench de Anotalo (spec §1): N frases de Multilingual LibriSpeech
(test, español), repartidas entre hablantes, como WAV mono 16 kHz de 16 bits + refs.json.

Uso: python -I scripts/bench/prep_clips.py <carpeta> [N=60]
Necesita pyarrow y soundfile (un entorno aparte basta: py -3.13 -m venv v; v\\Scripts\\pip install pyarrow soundfile numpy).
"""
import io
import json
import sys
import urllib.request
from pathlib import Path

try:
    import numpy as np
    import pyarrow.parquet as pq
    import soundfile as sf
except ImportError as e:  # pragma: no cover - mensaje para quien lo ejecuta
    raise SystemExit(f"Falta {e.name}: instala pyarrow, soundfile y numpy en el entorno que uses") from e

PARQUET_URL = "https://huggingface.co/api/datasets/facebook/multilingual_librispeech/parquet/spanish/test/0.parquet"

out = Path(sys.argv[1])
n = int(sys.argv[2]) if len(sys.argv) > 2 else 60
data = out / "data"
data.mkdir(parents=True, exist_ok=True)
src = data / "mls_es_test.parquet"
if not src.exists():
    part = src.with_suffix(".part")
    urllib.request.urlretrieve(PARQUET_URL, part)
    part.replace(src)

rows = pq.read_table(src).to_pylist()
by_speaker: dict = {}
for r in rows:
    by_speaker.setdefault(r.get("speaker_id"), []).append(r)
picked = []
i = 0
while len(picked) < n:
    progressed = False
    for _, lst in sorted(by_speaker.items(), key=lambda kv: str(kv[0])):
        if i < len(lst) and len(picked) < n:
            picked.append(lst[i])
            progressed = True
    if not progressed:
        break
    i += 1

refs, total = [], 0.0
for k, r in enumerate(picked):
    audio, sr = sf.read(io.BytesIO(r["audio"]["bytes"]), dtype="float32")
    if audio.ndim > 1:
        audio = audio.mean(axis=1)
    assert sr == 16000, sr
    name = f"u{k:03d}.wav"
    sf.write(out / name, audio, 16000, subtype="PCM_16")
    dur = len(audio) / sr
    total += dur
    refs.append({"file": name, "text": r["transcript"], "dur": round(dur, 2), "speaker": str(r.get("speaker_id"))})
(out / "refs.json").write_text(json.dumps(refs, ensure_ascii=False, indent=1), encoding="utf-8")
durs = [x["dur"] for x in refs]
print(f"{len(refs)} clips, {total / 60:.1f} min, min {min(durs):.1f}s max {max(durs):.1f}s, "
      f"speakers {len(set(x['speaker'] for x in refs))}")

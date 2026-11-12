"""Probabilidades de voz de la app de Python (calltx.vad.SileroVad) sobre jfk.wav, trama a trama.
Son la referencia del VAD de Anotalo (crates/core/tests/fixtures/vad_jfk.json).

Uso (desde la raíz del repo): .venv/Scripts/python scripts/fixtures/vad_reference.py
"""
import hashlib
import json
import sys
import wave
from pathlib import Path

import numpy as np

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root))
from calltx.vad import SileroVad, _model_path  # noqa: E402

with wave.open(str(root / "third_party" / "whisper.cpp" / "samples" / "jfk.wav")) as w:
    assert w.getframerate() == 16000 and w.getnchannels() == 1 and w.getsampwidth() == 2
    x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768.0

vad = SileroVad()
probs = [vad.prob(x[i * 512:(i + 1) * 512]) for i in range(len(x) // 512)]
sha = hashlib.sha256(Path(_model_path()).read_bytes()).hexdigest()
out = root / "crates" / "core" / "tests" / "fixtures" / "vad_jfk.json"
out.write_text(json.dumps({"sha256": sha, "probs": [round(p, 7) for p in probs]}, indent=0), encoding="utf-8")
print(len(probs))

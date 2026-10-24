"""Fábricas de modelos falsos para el proceso hijo de transcripción (las importa el hijo por nombre).

La marca del audio (audio[0]) elige el texto: 0.5 → "hola", 0.6 → alucinación, 0.9 → el hijo muere
la primera vez (si existe CALLTX_TEST_CRASH, apunta al archivo que marca que ya murió).
"""

import os
from pathlib import Path
from types import SimpleNamespace

TEXTS = {0.5: "hola", 0.6: "Gracias por ver", 0.7: "adiós", 0.8: "otra"}


class FakeModel:
    def transcribe(self, audio, **kw):
        key = round(float(audio[0]), 3) if len(audio) else 0.0
        if key == 0.9:
            mark = os.environ.get("CALLTX_TEST_CRASH")
            if mark and not Path(mark).exists():
                Path(mark).write_text("x")
                os._exit(1)
        if key == 0.95:
            os._exit(1)  # esta frase tumba al hijo siempre
        if key == 0.4:
            text = "larga " * 100  # respuestas grandes: llenan el pipe de vuelta
        else:
            text = TEXTS.get(key, "hola" if key == 0.9 else "")
        return iter([SimpleNamespace(text=" " + text, avg_logprob=-0.2, no_speech_prob=0.05)]), None


def _log_device(device: str) -> None:
    log = os.environ.get("CALLTX_TEST_DEVICES")
    if log:
        with open(log, "a", encoding="utf-8") as f:
            f.write(device + "\n")


def ok(model, device, compute_type, on_progress=None):
    _log_device(device)
    if on_progress is not None:
        on_progress(0.5, "Descargando modelo")
    return FakeModel()


def gpu_fails(model, device, compute_type, on_progress=None):
    _log_device(device)
    if device == "cuda":
        raise RuntimeError("CUDA no disponible")
    return FakeModel()


def always_fails(model, device, compute_type, on_progress=None):
    raise RuntimeError("no carga")

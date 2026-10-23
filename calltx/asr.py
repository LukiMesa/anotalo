"""Transcripción con faster-whisper: un único modelo caliente y un hilo de trabajo."""

from __future__ import annotations

import logging
import os
import queue
import re
import sys
import threading
from pathlib import Path
from typing import Any, Callable

import numpy as np

from .config import Settings
from .segmenter import Segment

log = logging.getLogger(__name__)

ResultCb = Callable[[Segment, str], None]

_BLOCKLIST = {
    "gracias por ver",
    "gracias por ver el video",
    "gracias por ver el vídeo",
    "subtítulos realizados por la comunidad de amaraorg",
    "subtitulos realizados por la comunidad de amaraorg",
    "suscríbete",
    "suscribete",
    "gracias",
    "música",
    "musica",
}


def _normalize(text: str) -> str:
    t = re.sub(r"[^\w\s]", "", text.lower())
    return " ".join(t.split())


def is_hallucination(text: str, avg_logprob: float, no_speech_prob: float) -> bool:
    norm = _normalize(text)
    if not norm:
        return True
    if no_speech_prob > 0.6 and avg_logprob < -1.0:
        return True
    return norm in _BLOCKLIST


def _add_cuda_dll_dirs() -> None:
    """Permite usar las DLL de CUDA instaladas como wheels de pip (sin CUDA del sistema)."""
    if sys.platform != "win32":
        return
    for base in map(Path, sys.path):
        nv = base / "nvidia"
        if not nv.is_dir():
            continue
        for sub in ("cublas", "cudnn", "cuda_nvrtc"):
            d = nv / sub / "bin"
            if d.is_dir():
                os.add_dll_directory(str(d))
                os.environ["PATH"] = str(d) + os.pathsep + os.environ.get("PATH", "")


def _default_factory(model: str, device: str, compute_type: str) -> Any:
    from faster_whisper import WhisperModel

    if device == "cuda":
        _add_cuda_dll_dirs()
    return WhisperModel(model, device=device, compute_type=compute_type)


class Transcriber:
    def __init__(self, settings: Settings, model_factory: Callable[..., Any] | None = None):
        self._s = settings
        self._factory = model_factory or _default_factory
        self._q: queue.Queue[tuple[Segment, ResultCb] | None] = queue.Queue()
        self._thread: threading.Thread | None = None
        self._model: Any = None
        self.device = settings.device
        self.fallback_used = False

    def start(self) -> None:
        try:
            self._model = self._factory(
                model=self._s.model, device=self._s.device, compute_type=self._s.compute_type
            )
            self.warmup()
        except Exception as e:  # noqa: BLE001 - cualquier fallo de CUDA → CPU
            log.warning("No se pudo usar %s (%s); paso a CPU con 'small'", self._s.device, e)
            self.fallback_used = True
            self.device = "cpu"
            self._model = self._factory(model="small", device="cpu", compute_type="int8")
            self.warmup()
        self._thread = threading.Thread(target=self._run, name="asr", daemon=True)
        self._thread.start()

    def warmup(self) -> None:
        self._transcribe(np.zeros(16000, dtype=np.float32))

    def submit(self, seg: Segment, on_result: ResultCb) -> None:
        self._q.put((seg, on_result))

    def wait_idle(self) -> None:
        """Bloquea hasta que todo lo enviado se haya transcrito."""
        self._q.join()

    def stop(self) -> None:
        if self._thread is None:
            return
        self._q.put(None)
        self._thread.join()
        self._thread = None

    def _transcribe(self, audio: np.ndarray) -> tuple[str, float, float]:
        segments, _info = self._model.transcribe(
            audio,
            language=self._s.language,
            beam_size=1,
            vad_filter=False,
            condition_on_previous_text=False,
            without_timestamps=True,
        )
        segs = list(segments)
        if not segs:
            return "", 0.0, 1.0
        text = "".join(s.text for s in segs).strip()
        logprob = float(np.mean([s.avg_logprob for s in segs]))
        nsp = float(np.mean([s.no_speech_prob for s in segs]))
        return text, logprob, nsp

    def _run(self) -> None:
        while True:
            item = self._q.get()
            try:
                if item is None:
                    return
                seg, cb = item
                try:
                    text, logprob, nsp = self._transcribe(seg.audio)
                except Exception:  # noqa: BLE001
                    log.exception("Error transcribiendo segmento de %s", seg.speaker)
                    continue
                if is_hallucination(text, logprob, nsp):
                    log.debug("Descartado (%s): %r", seg.speaker, text)
                    continue
                try:
                    cb(seg, text)
                except Exception:  # noqa: BLE001
                    log.exception("Error en callback de resultado")
            finally:
                self._q.task_done()

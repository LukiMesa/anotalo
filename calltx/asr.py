"""Transcripción con faster-whisper: un único modelo caliente y un hilo de trabajo."""

from __future__ import annotations

import logging
import os
import queue
import re
import sys
import threading
import time
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


# Frases típicas que Whisper inventa sobre ruido (vienen de subtítulos de YouTube).
_PATTERNS = [
    re.compile(r"\bsuscr[ií]bete\b"),
    re.compile(r"\bamara ?org\b"),
    re.compile(r"^gracias por (ver|vernos|su atención)"),
    re.compile(r"\bdale (a )?like\b"),
    re.compile(r"^subt[ií]tulos (realizados )?por\b"),
]


def _normalize(text: str) -> str:
    t = re.sub(r"[^\w\s]", "", text.lower())
    return " ".join(t.split())


def is_hallucination(text: str, avg_logprob: float, no_speech_prob: float) -> bool:
    norm = _normalize(text)
    if not norm:
        return True
    if no_speech_prob > 0.6 and avg_logprob < -1.0:
        return True
    if norm in _BLOCKLIST:
        return True
    return any(pat.search(norm) for pat in _PATTERNS)


def _add_cuda_dll_dirs() -> None:
    """Permite usar las DLL de CUDA instaladas como wheels de pip (sin CUDA del sistema)."""
    if sys.platform != "win32":
        return
    from . import paths

    # Versión instalada: DLL descargadas la primera vez en %LOCALAPPDATA%\Transcriptor\cuda.
    # La de desarrollo usa solo las de su propio entorno.
    root = paths.cuda_dir()
    if paths.is_frozen() and root.is_dir():
        for d in [p for p in root.iterdir() if p.is_dir() and not p.name.startswith("_")]:
            os.add_dll_directory(str(d))
            os.environ["PATH"] = str(d) + os.pathsep + os.environ.get("PATH", "")
    for base in map(Path, sys.path):
        nv = base / "nvidia"
        if not nv.is_dir():
            continue
        for sub in ("cublas", "cudnn", "cuda_nvrtc"):
            d = nv / sub / "bin"
            if d.is_dir():
                os.add_dll_directory(str(d))
                os.environ["PATH"] = str(d) + os.pathsep + os.environ.get("PATH", "")


def make_progress_bar(on_progress, model: str):
    """Clase tqdm que informa del progreso total (bytes de todos los archivos) sin escribir en
    consola: el ejecutable no tiene (sys.stderr es None)."""
    import io

    from tqdm.auto import tqdm

    bars: dict[int, tuple[float, float]] = {}

    class Bar(tqdm):
        def __init__(self, *a, **k):
            k["disable"] = False
            k["file"] = io.StringIO()
            super().__init__(*a, **k)

        def update(self, n=1):
            r = super().update(n)
            if getattr(self, "unit", "") == "B" and self.total:
                bars[id(self)] = (self.n, self.total)
                done = sum(v[0] for v in bars.values())
                tot = sum(v[1] for v in bars.values())
                if tot:
                    on_progress(min(0.99, done / tot), f"Descargando modelo {model}")
            return r

    return Bar


def _download_with_progress(model: str, on_progress) -> str:
    """Descarga el modelo de Hugging Face informando del progreso."""
    from faster_whisper.utils import _MODELS
    from huggingface_hub import snapshot_download

    repo = _MODELS.get(model, model)
    Bar = make_progress_bar(on_progress, model)
    allow = ["config.json", "preprocessor_config.json", "model.bin", "tokenizer.json", "vocabulary.*"]
    return snapshot_download(repo, allow_patterns=allow, tqdm_class=Bar)


def _default_factory(model: str, device: str, compute_type: str, on_progress=None) -> Any:
    from faster_whisper import WhisperModel

    if device == "cuda":
        _add_cuda_dll_dirs()
    try:
        # Sin red si el modelo ya está descargado.
        return WhisperModel(model, device=device, compute_type=compute_type, local_files_only=True)
    except Exception:  # noqa: BLE001 - no está en caché: descarga (solo la primera vez)
        log.info("Descargando modelo %s…", model)
        if on_progress is not None:
            path = _download_with_progress(model, on_progress)
            return WhisperModel(path, device=device, compute_type=compute_type)
        return WhisperModel(model, device=device, compute_type=compute_type)


class Transcriber:
    def __init__(self, settings: Settings, model_factory: Callable[..., Any] | None = None, resolved=None):
        from .device import has_nvidia_driver, resolve

        self._s = settings
        # Desarrollo: las DLL de CUDA vienen del entorno de pip, así que cuentan como listas.
        self.resolved = resolved or resolve(settings, has_nvidia_driver(), cuda_ready=True)
        self._factory = model_factory or _default_factory
        self._q: queue.Queue[tuple[Segment, ResultCb] | None] = queue.Queue()
        self._thread: threading.Thread | None = None
        self._model: Any = None
        self.device = self.resolved.device
        self.model_name = self.resolved.model
        self.compute_type = self.resolved.compute_type
        self.fallback_used = False

    def start(self, on_progress=None) -> None:
        self.load(on_progress)
        self._thread = threading.Thread(target=self._run, name="asr", daemon=True)
        self._thread.start()

    def load(self, on_progress=None) -> None:
        """Carga y calienta el modelo (con respaldo a CPU si la GPU falla), sin hilo de trabajo."""
        extra = {"on_progress": on_progress} if on_progress is not None else {}
        try:
            r = self.resolved
            self._model = self._factory(model=r.model, device=r.device, compute_type=r.compute_type, **extra)
            self.warmup()
        except Exception as e:  # noqa: BLE001 - cualquier fallo de CUDA → CPU
            if self.resolved.device == "cpu":
                raise
            from .device import cpu_fallback

            fb = cpu_fallback(self._s)
            log.warning("No se pudo usar la GPU (%s); paso al procesador con '%s'", e, fb.model)
            self.fallback_used = True
            self.device, self.model_name, self.compute_type = "cpu", fb.model, fb.compute_type
            self._model = self._factory(model=fb.model, device="cpu", compute_type=fb.compute_type, **extra)
            self.warmup()

    def warmup(self) -> None:
        self._transcribe(np.zeros(16000, dtype=np.float32))

    def transcribe_filtered(self, audio: np.ndarray) -> str | None:
        """Texto de una frase, o None si es una alucinación."""
        text, logprob, nsp = self._transcribe(audio)
        return None if is_hallucination(text, logprob, nsp) else text

    def submit(self, seg: Segment, on_result: ResultCb) -> None:
        self._q.put((seg, on_result))

    def wait_idle(self, timeout: float | None = None) -> bool:
        """Espera a que todo lo enviado se haya transcrito. False si vence el plazo."""
        deadline = None if timeout is None else time.monotonic() + timeout
        with self._q.all_tasks_done:
            while self._q.unfinished_tasks:
                if deadline is None:
                    self._q.all_tasks_done.wait()
                else:
                    left = deadline - time.monotonic()
                    if left <= 0:
                        log.warning("Quedan %d frases sin transcribir", self._q.unfinished_tasks)
                        return False
                    self._q.all_tasks_done.wait(left)
        return True

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

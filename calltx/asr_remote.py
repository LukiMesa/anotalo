"""Transcripción en un proceso aparte que se cierra en reposo.

Whisper + CUDA ocupan cientos de MB que Windows no devuelve mientras el proceso siga vivo, aunque
se libere el modelo. Por eso viven en un proceso hijo: se cierra tras `idle_s` segundos sin grabar
ni frases pendientes, y vuelve a arrancar al empezar a grabar (`warm`) o al llegar una frase.

Misma interfaz que `Transcriber` (la usan `ModelHolder` y `CallSession`).

Protocolo por el Pipe:
  padre → hijo: ("seg", id, audio) · ("stop",)
  hijo → padre: ("progress", p, texto) · ("ready", device, model, compute_type, fallback)
                · ("error", mensaje) · ("result", id, texto | None)
"""

from __future__ import annotations

import importlib
import logging
import multiprocessing as mp
import threading
import time
from dataclasses import asdict
from typing import Any, Callable

from .config import Settings
from .device import Resolved
from .segmenter import Segment

log = logging.getLogger(__name__)

ResultCb = Callable[[Segment, str], None]
MAX_RESPAWNS = 3  # muertes seguidas del hijo antes de descartar lo pendiente


# --- proceso hijo -------------------------------------------------------------
def _child_logging() -> None:
    from logging.handlers import RotatingFileHandler

    from .paths import data_dir

    try:
        base = data_dir()
        base.mkdir(parents=True, exist_ok=True)
        h = RotatingFileHandler(base / "asr-worker.log", maxBytes=1_000_000, backupCount=1, encoding="utf-8")
        h.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s: %(message)s"))
        root = logging.getLogger()
        root.setLevel(logging.INFO)
        root.addHandler(h)
    except OSError:
        pass


def _load_factory(path: str | None):
    if not path:
        return None
    mod, _, fn = path.partition(":")
    return getattr(importlib.import_module(mod), fn)


def worker_main(conn, settings_dict: dict, resolved: Resolved, factory_path: str | None) -> None:
    _child_logging()
    from .asr import Transcriber

    t = Transcriber(Settings(**settings_dict), model_factory=_load_factory(factory_path), resolved=resolved)
    try:
        t.load(on_progress=lambda p, m: conn.send(("progress", p, m)))
    except Exception as e:  # noqa: BLE001 - el padre decide qué hacer
        log.exception("No se pudo cargar el modelo")
        conn.send(("error", str(e) or type(e).__name__))
        return
    conn.send(("ready", t.device, t.model_name, t.compute_type, t.fallback_used))
    while True:
        try:
            msg = conn.recv()
        except (EOFError, OSError):
            return
        if msg[0] == "stop":
            return
        _, sid, audio = msg
        try:
            text = t.transcribe_filtered(audio)
        except Exception:  # noqa: BLE001
            log.exception("Error transcribiendo")
            text = None
        conn.send(("result", sid, text))


# --- lado de la app -----------------------------------------------------------
class RemoteTranscriber:
    def __init__(
        self,
        settings: Settings,
        resolved: Resolved,
        idle_s: float | None,
        busy: Callable[[], bool] = lambda: False,
        factory: str | None = None,
        poll_s: float = 1.0,
        on_change: Callable[[], None] | None = None,
    ):
        self._s = settings
        self.resolved = resolved
        self.idle_s = idle_s  # None = nunca se libera
        self._busy = busy
        self._factory = factory
        self._poll = poll_s
        self._on_change = on_change  # se avisa al cargar y al liberar (para la ventana)
        self._ctx = mp.get_context("spawn")
        self._cv = threading.Condition(threading.RLock())
        self._pending: dict[int, tuple[Segment, ResultCb]] = {}  # en orden de llegada
        self._next_id = 0
        self._state = "down"  # down | starting | up
        self._gen = 0  # cada hijo tiene su generación; los mensajes de hijos viejos se ignoran
        self._proc: Any = None
        self._conn: Any = None
        self._error: str | None = None
        self._deaths = 0
        self._last_used = time.monotonic()
        self._closed = False
        self._watcher: threading.Thread | None = None
        self.device = resolved.device
        self.model_name = resolved.model
        self.fallback_used = False

    @property
    def loaded(self) -> bool:
        return self._state == "up"

    # --- ciclo de vida ---------------------------------------------------------
    def start(self, on_progress=None) -> None:
        """Primera carga (descargas y elección GPU/CPU). Bloquea hasta que el hijo esté listo."""
        with self._cv:
            self._error = None
            self._spawn(on_progress)
            while self._state == "starting":
                self._cv.wait()
            if self._state != "up":
                raise RuntimeError(self._error or "El proceso de transcripción terminó al cargar")
        self._watcher = threading.Thread(target=self._watch, name="asr-idle", daemon=True)
        self._watcher.start()

    def warm(self) -> None:
        """Empieza una grabación: que el hijo vaya arrancando ya, sin esperar a la primera frase."""
        with self._cv:
            self._last_used = time.monotonic()
            if self._state == "down" and not self._closed:
                self._spawn()

    def submit(self, seg: Segment, on_result: ResultCb) -> None:
        with self._cv:
            sid = self._next_id
            self._next_id += 1
            self._pending[sid] = (seg, on_result)
            self._last_used = time.monotonic()
            if self._state == "up":
                self._send(("seg", sid, seg.audio))
            elif self._state == "down" and not self._closed:
                self._spawn()

    def wait_idle(self, timeout: float | None = None) -> bool:
        deadline = None if timeout is None else time.monotonic() + timeout
        with self._cv:
            while self._pending:
                left = None if deadline is None else deadline - time.monotonic()
                if left is not None and left <= 0:
                    log.warning("Quedan %d frases sin transcribir", len(self._pending))
                    return False
                self._cv.wait(left)
        return True

    def stop(self) -> None:
        with self._cv:
            self._closed = True
            self._release()
            self._cv.notify_all()
        if self._watcher is not None:
            self._watcher.join(5)
            self._watcher = None

    # --- interno (todo con self._cv tomado) -------------------------------------
    def _changed(self) -> None:
        cb = self._on_change
        if cb is not None:
            threading.Thread(target=cb, name="asr-changed", daemon=True).start()

    def _spawn(self, on_progress=None) -> None:
        self._gen += 1
        gen = self._gen
        parent, child = self._ctx.Pipe(duplex=True)
        proc = self._ctx.Process(
            target=worker_main,
            args=(child, asdict(self._s), self.resolved, self._factory),
            name="calltx-asr",
            daemon=True,
        )
        proc.start()
        child.close()
        self._proc, self._conn, self._state = proc, parent, "starting"
        log.info("Proceso de transcripción lanzado (%s, %s)", self.resolved.device, self.resolved.model)
        threading.Thread(target=self._read, args=(gen, parent, proc, on_progress),
                         name="asr-remote", daemon=True).start()

    def _send(self, msg) -> None:
        try:
            self._conn.send(msg)
        except (OSError, ValueError, AttributeError):
            pass  # el hijo murió: el lector lo detecta y reenvía lo pendiente

    def _release(self) -> None:
        if self._state == "down":
            return
        proc, conn = self._proc, self._conn
        self._gen += 1  # el lector del hijo viejo ya no cuenta
        self._state, self._proc, self._conn = "down", None, None
        self._changed()
        if conn is not None:
            try:
                conn.send(("stop",))
            except (OSError, ValueError):
                pass

        def finish():
            proc.join(5)
            if proc.is_alive():
                proc.kill()
            try:
                conn.close()
            except OSError:
                pass

        threading.Thread(target=finish, name="asr-release", daemon=True).start()

    def _on_ready(self, device, model, compute_type, fallback) -> None:
        self._state = "up"
        self._deaths = 0
        self._last_used = time.monotonic()  # recién cargado: cuenta atrás completa
        self.device, self.model_name = device, model
        self.fallback_used = self.fallback_used or fallback
        # Relanzar con lo que de verdad funcionó (tras un respaldo a CPU no se reintenta la GPU).
        self.resolved = Resolved(device, compute_type, model, needs_cuda=False)
        for sid, (seg, _cb) in self._pending.items():
            self._send(("seg", sid, seg.audio))
        self._changed()

    def _on_dead(self) -> None:
        was_up = self._state == "up"
        self._state, self._proc, self._conn = "down", None, None
        if was_up:
            self._changed()
        if self._closed or not (self._pending or self._busy()):
            return
        self._deaths += 1
        if self._deaths > MAX_RESPAWNS:
            log.error("El proceso de transcripción murió %d veces: descarto %d frases",
                      self._deaths, len(self._pending))
            self._pending.clear()
            self._deaths = 0
            return
        log.warning("El proceso de transcripción terminó; relanzando (%d)", self._deaths)
        self._spawn()

    def _read(self, gen: int, conn, proc, on_progress) -> None:
        while True:
            try:
                msg = conn.recv()
            except (EOFError, OSError):
                msg = None
            if msg is not None and msg[0] == "progress":
                if on_progress is not None:
                    try:
                        on_progress(msg[1], msg[2])
                    except Exception:  # noqa: BLE001
                        log.exception("Error informando progreso")
                continue
            callback = None
            with self._cv:
                if gen != self._gen:
                    return
                if msg is None:
                    was_starting = self._state == "starting"
                    if was_starting and self._error is None and self._watcher is None:
                        self._error = "El proceso de transcripción terminó al cargar"
                    self._on_dead()
                    self._cv.notify_all()
                    return
                kind = msg[0]
                if kind == "ready":
                    self._on_ready(*msg[1:])
                elif kind == "error":
                    self._error = msg[1]
                    self._state, self._proc, self._conn = "down", None, None
                    self._gen += 1
                    if self._watcher is not None and self._pending:
                        log.error("No se pudo recargar el modelo (%s): descarto %d frases",
                                  msg[1], len(self._pending))
                        self._pending.clear()
                    self._cv.notify_all()
                    return
                elif kind == "result":
                    item = self._pending.get(msg[1])
                    self._last_used = time.monotonic()
                    if item is not None and msg[2]:
                        callback = (item[1], item[0], msg[2])
                self._cv.notify_all()
            if kind == "result":
                if callback is not None:
                    cb, seg, text = callback
                    try:
                        cb(seg, text)
                    except Exception:  # noqa: BLE001
                        log.exception("Error en callback de resultado")
                # Se quita de pendientes después del aviso: wait_idle no vuelve antes de escribir la línea.
                with self._cv:
                    self._pending.pop(msg[1], None)
                    self._cv.notify_all()

    def _watch(self) -> None:
        while True:
            with self._cv:
                if self._closed:
                    return
                now = time.monotonic()
                if self._busy():
                    self._last_used = now  # la cuenta atrás empieza al terminar de grabar
                elif (self._state == "up" and self.idle_s is not None and not self._pending
                      and now - self._last_used >= self.idle_s):
                    log.info("Sin uso durante %.0f s: libero el proceso de transcripción", self.idle_s)
                    self._release()
                self._cv.wait(self._poll)


# --- autodiagnóstico del ejecutable ----------------------------------------------
class _EchoModel:
    def transcribe(self, audio, **kw):
        from types import SimpleNamespace

        return iter([SimpleNamespace(text=" ok", avg_logprob=-0.1, no_speech_prob=0.0)]), None


def _echo_factory(model, device, compute_type, on_progress=None):
    return _EchoModel()


def selftest_roundtrip(timeout: float = 60) -> str:
    """Lanza un proceso hijo de verdad (con un modelo de eco) y devuelve el texto que contesta.
    Comprueba que el ejecutable empaquetado puede arrancar su proceso de transcripción."""
    import numpy as np

    t = RemoteTranscriber(Settings(), Resolved("cpu", "int8", "eco", needs_cuda=False), idle_s=None,
                          factory="calltx.asr_remote:_echo_factory")
    out: list[str] = []
    try:
        t.start()
        t.submit(Segment("Yo", 0.0, 0.1, np.full(1600, 0.1, np.float32)), lambda s, text: out.append(text))
        if not t.wait_idle(timeout):
            raise RuntimeError("el proceso de transcripción no contestó")
    finally:
        t.stop()
    return out[0] if out else ""

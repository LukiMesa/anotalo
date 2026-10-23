"""Una llamada en curso: dos fuentes → dos segmentadores → transcriptor → Markdown."""

from __future__ import annotations

import logging
import queue
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Callable

import numpy as np

from .audio.base import AudioSource
from .config import Settings
from .segmenter import Segment, Segmenter
from .transcript import TranscriptWriter, fmt_elapsed
from .vad import SileroVad

log = logging.getLogger(__name__)

OnLine = Callable[[str, str, float, str], None]  # speaker, "HH:MM:SS", start_s, text
MAX_RETRIES = 3


class _Lane:
    """Una fuente con su cola y su hilo de proceso (el callback de audio nunca bloquea)."""

    def __init__(self, source: AudioSource, segmenter: Segmenter, t0: float):
        self.source = source
        self.segmenter = segmenter
        self.t0 = t0
        self.q: queue.Queue[tuple[float, np.ndarray] | None] = queue.Queue()
        self.thread = threading.Thread(target=self._run, name=f"lane-{source.name}", daemon=True)
        self.failures = 0

    def on_audio(self, t: float, x: np.ndarray) -> None:
        self.q.put((t, x))

    def _run(self) -> None:
        while True:
            item = self.q.get()
            if item is None:
                self.segmenter.flush()
                return
            t, x = item
            try:
                self.segmenter.feed(t - self.t0, x)
            except Exception:  # noqa: BLE001
                log.exception("Error procesando audio de %s", self.source.name)


class CallSession:
    def __init__(
        self,
        settings: Settings,
        transcriber,
        app_label: str,
        mic: AudioSource,
        others: AudioSource,
        on_line: OnLine | None = None,
        vad_factory: Callable[[], Callable[[np.ndarray], float]] = SileroVad,
        clock: Callable[[], float] = time.monotonic,
        retry_delay_s: float = 1.0,
    ):
        self._s = settings
        self._asr = transcriber
        self.app_label = app_label
        self._sources = [(mic, settings.label_me), (others, settings.label_others)]
        self._on_line = on_line
        self._vad_factory = vad_factory
        self._clock = clock
        self._retry_delay = retry_delay_s
        self._lanes: list[_Lane] = []
        self._stop_evt = threading.Event()
        self._timer: threading.Thread | None = None
        self.writer: TranscriptWriter | None = None
        self.started_at: datetime | None = None

    @property
    def path(self) -> Path | None:
        return self.writer.path if self.writer else None

    def start(self) -> None:
        self.started_at = datetime.now()
        t0 = self._clock()
        self.writer = TranscriptWriter(Path(self._s.output_dir), self.app_label, self.started_at)
        for source, label in self._sources:
            seg = Segmenter(
                label,
                self._vad_factory(),
                self._on_segment,
                silence_ms=self._s.silence_ms,
                max_segment_s=self._s.max_segment_s,
                min_segment_ms=self._s.min_segment_ms,
            )
            lane = _Lane(source, seg, t0)
            lane.thread.start()
            self._lanes.append(lane)
            self._open(lane)
        self._timer = threading.Thread(target=self._tick_loop, name="session-timer", daemon=True)
        self._timer.start()

    def _open(self, lane: _Lane) -> bool:
        while lane.failures <= MAX_RETRIES:
            try:
                lane.source.start(lane.on_audio)
                return True
            except Exception as e:  # noqa: BLE001
                lane.failures += 1
                log.warning("No se pudo abrir %s (%s), intento %d", lane.source.name, e, lane.failures)
                if self._stop_evt.wait(self._retry_delay * lane.failures):
                    return False
        log.error("Fuente %s descartada tras %d intentos", lane.source.name, MAX_RETRIES)
        return False

    def _tick_loop(self) -> None:
        while not self._stop_evt.wait(0.5):
            if self.writer:
                self.writer.flush_due()
            for lane in self._lanes:
                active = getattr(lane.source, "is_active", None)
                if active is not None and not active() and lane.failures <= MAX_RETRIES:
                    log.warning("La fuente %s se detuvo; reabriendo", lane.source.name)
                    try:
                        lane.source.stop()
                    except Exception:  # noqa: BLE001
                        pass
                    lane.failures += 1
                    self._open(lane)

    def _on_segment(self, seg: Segment) -> None:
        self._asr.submit(seg, self._on_result)

    def _on_result(self, seg: Segment, text: str) -> None:
        w = self.writer
        if w is None or w.add(seg.speaker, seg.start_s, text, end_s=seg.end_s) is None:
            return
        if self._on_line:
            try:
                self._on_line(seg.speaker, fmt_elapsed(seg.start_s), seg.start_s, text)
            except Exception:  # noqa: BLE001
                log.exception("Error notificando línea")

    def stop(self) -> Path:
        self._stop_evt.set()
        if self._timer:
            self._timer.join(3)
        for lane in self._lanes:
            try:
                lane.source.stop()
            except Exception:  # noqa: BLE001
                log.exception("Error parando %s", lane.source.name)
            lane.q.put(None)
        for lane in self._lanes:
            lane.thread.join(10)
        self._asr.wait_idle()
        assert self.writer is not None
        return self.writer.close(datetime.now())

    def text(self) -> str:
        return self.writer.text() if self.writer else ""

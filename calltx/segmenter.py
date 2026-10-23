"""Corta un flujo de audio en frases usando un detector de voz por tramas."""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass
from typing import Callable

import numpy as np

SR = 16000
FRAME = 512
FRAME_S = FRAME / SR
GAP_TOLERANCE_S = 0.05
TRAILING_KEEP_S = 0.2


@dataclass
class Segment:
    speaker: str
    start_s: float
    end_s: float
    audio: np.ndarray


class Segmenter:
    def __init__(
        self,
        speaker: str,
        vad_prob: Callable[[np.ndarray], float],
        on_segment: Callable[[Segment], None],
        silence_ms: int = 600,
        max_segment_s: float = 25.0,
        min_segment_ms: int = 300,
        threshold: float = 0.5,
        pre_roll_ms: int = 200,
    ):
        self.speaker = speaker
        self._vad = vad_prob
        self._emit = on_segment
        self._silence_s = silence_ms / 1000
        self._max_s = max_segment_s
        self._min_voiced_s = min_segment_ms / 1000
        self._on = threshold
        self._off = threshold - 0.15
        self._preroll: deque[tuple[float, np.ndarray]] = deque(maxlen=max(1, round(pre_roll_ms / 1000 / FRAME_S)))
        self._buf = np.zeros(0, dtype=np.float32)
        self._buf_t: float | None = None
        self._reset_segment()

    def _reset_segment(self) -> None:
        self._frames: list[np.ndarray] = []
        self._seg_start = 0.0
        self._voiced_s = 0.0
        self._silence_run = 0.0
        self._in_speech = False

    # --- entrada -------------------------------------------------------
    def feed(self, t_s: float, samples: np.ndarray) -> None:
        if len(samples) == 0:
            return
        if self._buf_t is None:
            self._buf_t = t_s
        else:
            expected = self._buf_t + len(self._buf) / SR
            gap = t_s - expected
            if gap > GAP_TOLERANCE_S:
                self._on_gap(gap)
                self._buf = np.zeros(0, dtype=np.float32)
                self._buf_t = t_s
        self._buf = np.concatenate([self._buf, np.asarray(samples, dtype=np.float32)])
        n = len(self._buf) // FRAME
        for i in range(n):
            frame = self._buf[i * FRAME : (i + 1) * FRAME]
            self._process(self._buf_t + i * FRAME_S, frame)
        if n:
            self._buf = self._buf[n * FRAME :]
            self._buf_t += n * FRAME_S

    def flush(self) -> None:
        if self._in_speech:
            self._finish()
        self._preroll.clear()

    # --- lógica ----------------------------------------------------------
    def _on_gap(self, gap: float) -> None:
        """La captura de sistema no entrega paquetes en silencio: el hueco cuenta como silencio."""
        self._preroll.clear()
        if not self._in_speech:
            return
        self._silence_run += gap
        if self._silence_run >= self._silence_s:
            self._finish()

    def _process(self, t: float, frame: np.ndarray) -> None:
        p = self._vad(frame)
        if not self._in_speech:
            if p >= self._on:
                self._in_speech = True
                pre = list(self._preroll)
                self._preroll.clear()
                self._seg_start = pre[0][0] if pre else t
                self._frames = [f for _, f in pre] + [frame]
                self._voiced_s = FRAME_S
                self._silence_run = 0.0
            else:
                self._preroll.append((t, frame))
            return
        self._frames.append(frame)
        if p < self._off:
            self._silence_run += FRAME_S
        else:
            self._silence_run = 0.0
            self._voiced_s += FRAME_S
        if self._silence_run >= self._silence_s:
            self._finish()
        elif len(self._frames) * FRAME_S >= self._max_s:
            self._finish(keep_speaking=True)

    def _finish(self, keep_speaking: bool = False) -> None:
        frames = self._frames
        # recorta el silencio final dejando un pequeño margen
        silent_frames = min(len(frames), int(self._silence_run / FRAME_S + 1e-9))
        drop = max(0, silent_frames - round(TRAILING_KEEP_S / FRAME_S))
        if drop:
            frames = frames[: len(frames) - drop]
        start = self._seg_start
        voiced = self._voiced_s
        self._reset_segment()
        if keep_speaking:
            self._in_speech = True
            self._seg_start = start + len(frames) * FRAME_S
        if not frames or voiced < self._min_voiced_s:
            return
        audio = np.concatenate(frames)
        self._emit(Segment(self.speaker, start, start + len(audio) / SR, audio))

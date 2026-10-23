"""Escritura incremental de la transcripción en Markdown.

Las líneas de los dos audios pueden terminar de transcribirse en desorden, así
que se retienen HOLD_S segundos y se escriben ordenadas por su inicio. Cada
escritura hace fsync: si la app se cae, lo ya escrito queda en disco.
"""

from __future__ import annotations

import os
import threading
import time
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Callable

HOLD_S = 3.0
MERGE_GAP_S = 2.0


def fmt_elapsed(seconds: float) -> str:
    s = max(0, int(seconds))
    return f"{s // 3600:02d}:{s % 3600 // 60:02d}:{s % 60:02d}"


@dataclass
class _Pending:
    speaker: str
    start_s: float
    end_s: float
    text: str
    arrived: float

    def render(self) -> str:
        return f"**[{fmt_elapsed(self.start_s)}] {self.speaker}:** {self.text}"


class TranscriptWriter:
    def __init__(
        self,
        output_dir: Path,
        app: str,
        started_at: datetime,
        clock: Callable[[], float] = time.monotonic,
    ):
        self._dir = Path(output_dir)
        self._dir.mkdir(parents=True, exist_ok=True)
        self._app = app
        self._started_at = started_at
        self._clock = clock
        self._lock = threading.Lock()
        self._pending: list[_Pending] = []
        self._written: list[str] = []
        self._closed = False
        self.path = self._unique_path()
        self._file = open(self.path, "w", encoding="utf-8", newline="\n")
        self._write(self._header(None))

    def _unique_path(self) -> Path:
        stem = f"{self._started_at:%Y-%m-%d_%H%M}_{self._app}"
        p = self._dir / f"{stem}.md"
        n = 2
        while p.exists():
            p = self._dir / f"{stem}_{n}.md"
            n += 1
        return p

    def _header(self, ended_at: datetime | None) -> str:
        lines = [
            f"# Llamada — {self._app} — {self._started_at:%Y-%m-%d %H:%M}",
            "",
            f"- Inicio: {self._started_at:%H:%M:%S}",
        ]
        if ended_at is not None:
            dur = (ended_at - self._started_at).total_seconds()
            lines += [f"- Fin: {ended_at:%H:%M:%S}", f"- Duración: {fmt_elapsed(dur)}"]
        lines += ["", "---", "", ""]
        return "\n".join(lines)

    def _write(self, text: str) -> None:
        self._file.write(text)
        self._file.flush()
        os.fsync(self._file.fileno())

    def add(self, speaker: str, start_s: float, text: str, end_s: float | None = None) -> str | None:
        text = " ".join(text.split())
        if not text:
            return None
        end_s = start_s if end_s is None else end_s
        with self._lock:
            if self._closed:
                return None
            last = max(self._pending, key=lambda p: p.start_s, default=None)
            if (
                last is not None
                and last.speaker == speaker
                and start_s >= last.start_s
                and start_s - last.end_s < MERGE_GAP_S
            ):
                last.text = f"{last.text} {text}"
                last.end_s = max(last.end_s, end_s)
                last.arrived = self._clock()
            else:
                self._pending.append(_Pending(speaker, start_s, end_s, text, self._clock()))
        return f"**[{fmt_elapsed(start_s)}] {speaker}:** {text}"

    def flush_due(self) -> None:
        with self._lock:
            if self._closed or not self._pending:
                return
            now = self._clock()
            self._pending.sort(key=lambda p: p.start_s)
            # Escribe en orden solo el prefijo cuyas líneas ya cumplieron la espera.
            n = 0
            while n < len(self._pending) and now - self._pending[n].arrived >= HOLD_S:
                n += 1
            if n:
                self._emit(self._pending[:n])
                del self._pending[:n]

    def _emit(self, items: list[_Pending]) -> None:
        rendered = [p.render() for p in items]
        self._written += rendered
        self._write("".join(r + "\n\n" for r in rendered))

    def close(self, ended_at: datetime) -> Path:
        with self._lock:
            if self._closed:
                return self.path
            self._pending.sort(key=lambda p: p.start_s)
            self._emit(self._pending)
            self._pending.clear()
            self._file.close()
            body = "".join(r + "\n\n" for r in self._written)
            tmp = self.path.with_suffix(".tmp")
            tmp.write_text(self._header(ended_at) + body, encoding="utf-8", newline="\n")
            os.replace(tmp, self.path)
            self._closed = True
            return self.path

    def text(self) -> str:
        """Transcripción completa hasta ahora (escrita + pendiente), para copiar."""
        with self._lock:
            pend = sorted(self._pending, key=lambda p: p.start_s)
            return "\n\n".join(self._written + [p.render() for p in pend])

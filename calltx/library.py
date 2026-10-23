"""Las grabaciones son los Markdown de la carpeta de salida: aquí se leen, buscan y borran."""

from __future__ import annotations

import os
import re
import time
import unicodedata
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path
from typing import Literal

NOTE_APPS = {"Nota", "Manual"}
TRASH_DIR = ".papelera"

_NAME = re.compile(r"^(\d{4})-(\d{2})-(\d{2})_(\d{2})(\d{2})_([^_.]+)(?:_\d+)?\.md$")
_TITLE = re.compile(r"^# Llamada — (.+?) — ")
_FIELD = re.compile(r"^- (Inicio|Fin|Duración): (\d{2}):(\d{2}):(\d{2})\s*$")
_LINE = re.compile(r"^\*\*\[(\d{2}):(\d{2}):(\d{2})\] (.+?):\*\* (.*)$")


@dataclass
class Line:
    ts: str
    start_s: int
    speaker: str
    text: str


@dataclass
class Recording:
    id: str
    path: Path
    kind: Literal["call", "note"]
    app: str
    started_at: datetime
    ended_at: datetime | None
    duration_s: int | None
    lines: list[Line] = field(default_factory=list)
    live: bool = False

    @property
    def snippet(self) -> str:
        others = [l.text for l in self.lines if l.speaker != "Yo"]
        text = " ".join(others or [l.text for l in self.lines])
        return text[:160]


def _fold(s: str) -> str:
    s = unicodedata.normalize("NFKD", s.lower())
    return "".join(c for c in s if not unicodedata.combining(c))


def parse_recording(path: Path) -> Recording:
    path = Path(path)
    m = _NAME.match(path.name)
    app = m.group(6) if m else ""
    day = datetime(int(m[1]), int(m[2]), int(m[3])) if m else None
    started = day.replace(hour=int(m[4]), minute=int(m[5])) if m else datetime.min
    ended: datetime | None = None
    duration: int | None = None
    lines: list[Line] = []
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        text = ""
    for raw in text.splitlines():
        if (t := _TITLE.match(raw)) and not app:
            app = t.group(1)
        elif f := _FIELD.match(raw):
            h, mi, s = int(f[2]), int(f[3]), int(f[4])
            if f[1] == "Inicio" and day:
                started = day.replace(hour=h, minute=mi, second=s)
            elif f[1] == "Fin" and day:
                ended = day.replace(hour=h, minute=mi, second=s)
            elif f[1] == "Duración":
                duration = h * 3600 + mi * 60 + s
        elif l := _LINE.match(raw):
            start_s = int(l[1]) * 3600 + int(l[2]) * 60 + int(l[3])
            lines.append(Line(f"{l[1]}:{l[2]}:{l[3]}", start_s, l[4], l[5].strip()))
    if ended is not None and ended < started:
        ended += timedelta(days=1)
    kind: Literal["call", "note"] = "note" if app in NOTE_APPS else "call"
    return Recording(path.name, path, kind, app, started, ended, duration, lines)


def list_recordings(folder: Path, kind: str = "all", query: str = "") -> list[Recording]:
    folder = Path(folder)
    if not folder.is_dir():
        return []
    recs = [parse_recording(p) for p in folder.glob("*.md") if p.is_file()]
    if kind in ("call", "note"):
        recs = [r for r in recs if r.kind == kind]
    q = _fold(query.strip())
    if q:
        recs = [r for r in recs if any(q in _fold(l.text) for l in r.lines)]
    recs.sort(key=lambda r: (r.started_at, r.id), reverse=True)
    return recs


def to_plain_text(rec: Recording) -> str:
    return "\n\n".join(l.text for l in rec.lines if l.text)


def to_readable(rec: Recording) -> str:
    return "\n".join(f"[{l.ts}] {l.speaker}: {l.text}" for l in rec.lines)


class Trash:
    """Borrar = mover a una carpeta oculta; deshacer = devolverlo."""

    def __init__(self, folder: Path):
        self.folder = Path(folder)
        self.dir = self.folder / TRASH_DIR

    def delete(self, path: Path) -> str:
        path = Path(path)
        self.dir.mkdir(exist_ok=True)
        if os.name == "nt":
            import ctypes

            ctypes.windll.kernel32.SetFileAttributesW(str(self.dir), 0x2)  # oculta
        token = f"{int(time.time() * 1000)}__{path.name}"
        os.replace(path, self.dir / token)
        return token

    def undo(self, token: str) -> Path:
        src = self.dir / token
        name = token.split("__", 1)[1]
        dest = self.folder / name
        n = 2
        while dest.exists():
            dest = self.folder / f"{Path(name).stem}_{n}.md"
            n += 1
        os.replace(src, dest)
        return dest

    def purge(self, older_than_s: float = 3600) -> int:
        if not self.dir.is_dir():
            return 0
        limit = (time.time() - older_than_s) * 1000
        n = 0
        for p in self.dir.iterdir():
            ts = p.name.split("__", 1)[0]
            if ts.isdigit() and int(ts) < limit:
                p.unlink(missing_ok=True)
                n += 1
        return n

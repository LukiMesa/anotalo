"""Elección de fuentes de audio para una llamada."""

from __future__ import annotations

import logging

import psutil

from .base import AudioSource
from .pa import DefaultLoopbackSource, MicSource
from .process_loopback import ProcessLoopbackSource

log = logging.getLogger(__name__)

__all__ = ["MicSource", "DefaultLoopbackSource", "ProcessLoopbackSource", "find_root_pid", "open_call_audio"]


def find_root_pid(exe_name: str) -> int | None:
    """PID del proceso raíz de una app multiproceso (p. ej. el firefox.exe padre)."""
    exe = exe_name.lower()
    procs = {p.info["pid"]: p.info for p in psutil.process_iter(["pid", "name", "ppid"])}
    roots = [
        info["pid"]
        for info in procs.values()
        if (info.get("name") or "").lower() == exe
        and ((procs.get(info.get("ppid")) or {}).get("name") or "").lower() != exe
    ]
    return min(roots) if roots else None


def open_call_audio(pid: int | None) -> AudioSource:
    if pid:
        src = ProcessLoopbackSource(pid)
        try:
            src.probe()
            return src
        except Exception as e:  # noqa: BLE001
            log.warning("Process loopback no disponible para pid %s (%s); uso el dispositivo por defecto", pid, e)
    return DefaultLoopbackSource()

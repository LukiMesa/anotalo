"""Elección de fuentes de audio para una llamada."""

from __future__ import annotations

import logging

import psutil

from . import coreaudio
from .base import AudioSource
from .pa import DefaultLoopbackSource, MicSource
from .process_loopback import EndpointLoopbackSource, ProcessLoopbackSource
from .routing import pick_capture, pick_render

log = logging.getLogger(__name__)

__all__ = ["MicSource", "DefaultLoopbackSource", "ProcessLoopbackSource", "EndpointLoopbackSource",
           "find_root_pid", "pick_others", "pick_mic"]


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


def pick_others(spec, render_snapshot=coreaudio.render_snapshot) -> AudioSource:
    """De dónde sale la voz de "los demás". Se decide en el momento (y otra vez si la fuente se cae):
    el audio del programa si se puede; si no, el dispositivo de salida donde suena la llamada."""
    if spec.kind == "process" and spec.exe:
        pid = find_root_pid(spec.exe)
        if pid:
            src = ProcessLoopbackSource(pid)
            try:
                src.probe()
                return src
            except Exception as e:  # noqa: BLE001
                log.warning("Sin audio del proceso %s (%s); uso su dispositivo de salida", spec.exe, e)
    try:
        snap = render_snapshot()
        ep, why = pick_render(snap.sessions, spec.platform, snap.comm, snap.console)
    except Exception as e:  # noqa: BLE001
        log.warning("No se pudo leer el audio de Windows (%s); uso la salida predeterminada", e)
        return DefaultLoopbackSource()
    if ep is None:
        return DefaultLoopbackSource()
    name = snap.names.get(ep, "")
    log.info("Otros: %s (%s)", name or ep, why)
    return EndpointLoopbackSource(ep, name)


def pick_mic(platform: str | None, mic_setting: str | None,
             capture_snapshot=coreaudio.capture_snapshot) -> MicSource:
    """Micrófono para "Yo": el elegido en Ajustes; si no, el que usa la app de la llamada;
    si no, el predeterminado de este momento."""
    if mic_setting:
        return MicSource(mic_setting)
    try:
        snap = capture_snapshot()
        ep, why = pick_capture(snap.sessions, platform, snap.console)
    except Exception as e:  # noqa: BLE001
        log.warning("No se pudo leer el audio de Windows (%s); uso el micrófono predeterminado", e)
        return MicSource(None)
    name = snap.names.get(ep) if ep else None
    log.info("Yo: %s (%s)", name or "predeterminado", why)
    return MicSource(name)

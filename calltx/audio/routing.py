"""Qué dispositivo grabar: el que usa de verdad la app de la llamada.

Windows dice qué programas tienen sonido abierto en cada dispositivo (sesiones de audio).
Con auriculares Bluetooth, durante una llamada la app suena en el dispositivo "manos libres",
que no suele ser el predeterminado: hay que seguir a la app, no al predeterminado.
"""

from __future__ import annotations

from dataclasses import dataclass

from ..platforms import platform_for_key


@dataclass(frozen=True)
class SessionInfo:
    endpoint_id: str
    endpoint_name: str
    pid: int
    active: bool
    key: str | None  # como en el registro del micrófono: paquete o "NonPackaged/#app.exe"


def process_key(exe: str | None, package: str | None) -> str | None:
    """Clave al estilo del registro del micrófono, para reconocer la plataforma."""
    if package:
        return package
    if exe:
        return f"NonPackaged/#{exe.lower()}"
    return None


def _call_app(info: SessionInfo, platform_key: str | None):
    p = platform_for_key(info.key) if info.key else None
    if p is None or (platform_key is not None and p.key != platform_key):
        return None
    return p


def _prefer(endpoints: list[str], *preferred: str | None) -> str:
    for p in preferred:
        if p and p in endpoints:
            return p
    return endpoints[0]


def _unique(items) -> list[str]:
    return list(dict.fromkeys(items))


def pick_render(sessions: list[SessionInfo], platform_key: str | None,
                comm: str | None, console: str | None) -> tuple[str | None, str]:
    """Dispositivo de salida donde escuchar a "los demás" y por qué.

    platform_key: la plataforma de la llamada, o None (grabación a mano: cualquier app de llamadas).
    comm / console: dispositivos predeterminados de comunicaciones y general.
    """
    active = [x for x in sessions if x.active]
    hits = [x for x in active if _call_app(x, platform_key)]
    if hits:
        ep = _prefer(_unique(x.endpoint_id for x in hits), comm, console)
        name = _call_app(next(x for x in hits if x.endpoint_id == ep), platform_key).name
        return ep, f"donde suena {name}"
    sounding = _unique(x.endpoint_id for x in active)
    if sounding:
        ep = _prefer(sounding, comm, console)
        return ep, "con sonido" if ep not in (comm, console) else "predeterminado con sonido"
    ep = console or comm
    return ep, "predeterminado"


def pick_capture(sessions: list[SessionInfo], platform_key: str | None,
                 console: str | None) -> tuple[str | None, str]:
    """Micrófono para "Yo": el que usa la app de la llamada; si no se sabe, el predeterminado."""
    hits = [x for x in sessions if x.active and _call_app(x, platform_key)]
    if hits:
        ep = _prefer(_unique(x.endpoint_id for x in hits), console)
        return ep, f"el que usa {_call_app(hits[0], platform_key).name}"
    return console, "predeterminado"

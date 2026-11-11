"""Plataformas de llamada que se detectan solas.

Windows anota qué apps usan el micrófono (ver detector.py). Aquí se decide cuáles de
esas apps son de llamadas: una lista conocida de apps de escritorio y navegadores. Cualquier
otra (juegos, apps de dictado, esta misma app) nunca cuenta como llamada.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Literal


@dataclass(frozen=True)
class Platform:
    key: str
    name: str  # nombre que se ve en la grabación ("Llamada de Zoom")
    setting: str  # ajuste que la activa (Settings.<setting>)
    exes: tuple[str, ...] = ()  # procesos (en minúsculas)
    packaged: tuple[str, ...] = ()  # prefijo de la app empaquetada en el registro (en minúsculas)
    audio: Literal["process", "device"] = "process"  # de dónde sale la voz de los demás
    start_hold_s: float | None = None  # segundos de micro antes de contar como llamada (None = el general)
    browser: bool = False


PLATFORMS: tuple[Platform, ...] = (
    # Capturar solo el proceso de Teams da silencio en algunos PCs (fallo conocido de Windows):
    # se graba el dispositivo donde suena Teams.
    Platform("teams", "Teams", "detect_teams", ("ms-teams.exe", "teams.exe"),
             ("msteams_", "microsoftteams_"), audio="device"),
    Platform("zoom", "Zoom", "detect_zoom", ("zoom.exe",)),
    # WhatsApp usa el micro también al grabar notas de voz y su ventana no cambia (prueba
    # real): solo cuenta tras 30 s seguidos. Se pierden los primeros 30 s de la llamada.
    # Suena desde procesos de WebView2 fuera de su árbol: se graba el audio del equipo.
    Platform("whatsapp", "WhatsApp", "detect_whatsapp", ("whatsapp.root.exe", "whatsapp.exe"),
             ("5319275a.whatsappdesktop_",), audio="device", start_hold_s=30.0),
    Platform("slack", "Slack", "detect_slack", ("slack.exe",), ("91750d7e.slack_",)),
    Platform("discord", "Discord", "detect_discord", ("discord.exe",)),
    Platform("firefox", "Firefox", "detect_browsers", ("firefox.exe",), browser=True),
    Platform("chrome", "Chrome", "detect_browsers", ("chrome.exe",), browser=True),
    Platform("brave", "Brave", "detect_browsers", ("brave.exe",), browser=True),
    Platform("edge", "Edge", "detect_browsers", ("msedge.exe",), browser=True),
    Platform("opera", "Opera", "detect_browsers", ("opera.exe",), browser=True),
    Platform("vivaldi", "Vivaldi", "detect_browsers", ("vivaldi.exe",), browser=True),
)


def platform_for_key(registry_key: str) -> Platform | None:
    """Plataforma de una entrada del registro del micrófono (empaquetada o "NonPackaged/…#app.exe")."""
    k = registry_key.lower()
    if k.startswith("nonpackaged/"):
        exe = k.rsplit("#", 1)[-1]
        return next((p for p in PLATFORMS if exe in p.exes), None)
    return next((p for p in PLATFORMS if any(k.startswith(pre) for pre in p.packaged)), None)


_SITES: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("Meet", re.compile(r"^Meet\s*[-–—:]|Google Meet")),
    ("Teams", re.compile(r"Microsoft Teams")),
    ("Zoom", re.compile(r"\bZoom\b")),
    ("WhatsApp", re.compile(r"\bWhatsApp\b")),
    ("Discord", re.compile(r"\bDiscord\b")),
    ("Slack", re.compile(r"\bSlack\b")),
)


def site_from_title(title: str) -> str | None:
    """Plataforma reconocible en el título de una ventana del navegador, o None."""
    for name, rx in _SITES:
        if rx.search(title):
            return name
    return None

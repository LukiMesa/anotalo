"""Arranque con Windows: acceso directo en la carpeta Inicio del usuario."""

from __future__ import annotations

import os
from pathlib import Path

SHORTCUT_NAME = "Transcriptor de llamadas.lnk"


def _startup_dir() -> Path:
    return Path(os.environ["APPDATA"]) / "Microsoft" / "Windows" / "Start Menu" / "Programs" / "Startup"


def shortcut_path(startup_dir: Path | None = None) -> Path:
    return Path(startup_dir or _startup_dir()) / SHORTCUT_NAME


def enable(target: Path, args: str, workdir: Path, startup_dir: Path | None = None) -> None:
    import comtypes.client

    lnk = shortcut_path(startup_dir)
    lnk.parent.mkdir(parents=True, exist_ok=True)
    shell = comtypes.client.CreateObject("WScript.Shell", dynamic=True)
    sc = shell.CreateShortcut(str(lnk))
    sc.TargetPath = str(target)
    sc.Arguments = args
    sc.WorkingDirectory = str(workdir)
    sc.Description = "Transcriptor de llamadas"
    sc.Save()


def disable(startup_dir: Path | None = None) -> None:
    shortcut_path(startup_dir).unlink(missing_ok=True)


def is_enabled(startup_dir: Path | None = None) -> bool:
    return shortcut_path(startup_dir).exists()


def ensure_autostart(settings, target: Path, args: str, workdir: Path, startup_dir: Path | None = None) -> bool:
    """Primera vez con esta versión: crea (o rehace) el acceso directo. Devuelve True si cambió settings."""
    if settings.autostart_configured:
        return False
    if settings.autostart:
        enable(target, args, workdir, startup_dir)
    else:
        disable(startup_dir)
    settings.autostart_configured = True
    return True

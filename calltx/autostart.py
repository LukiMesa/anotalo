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


def target_for_current_run(frozen: bool | None = None) -> tuple[Path, str, Path] | None:
    """(ejecutable, argumentos, carpeta) para el acceso directo. En desarrollo: None (no se toca)."""
    import sys

    from .paths import is_frozen

    if not (is_frozen() if frozen is None else frozen):
        return None
    exe = Path(sys.executable)
    return exe, "--minimized", exe.parent


def ensure_autostart(settings, target: Path, args: str, workdir: Path, startup_dir: Path | None = None,
                     frozen: bool | None = None) -> bool:
    """Primera vez de la versión instalada: el ajuste refleja lo que eligió el instalador
    (si creó o no el acceso directo). En desarrollo nunca se toca nada. True si cambió settings."""
    from .paths import is_frozen

    if not (is_frozen() if frozen is None else frozen):
        return False
    if settings.autostart_configured:
        return False
    settings.autostart = is_enabled(startup_dir)
    settings.autostart_configured = True
    return True

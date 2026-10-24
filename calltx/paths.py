"""Rutas de la app. La versión instalada (empaquetada) y la de desarrollo (código fuente)
usan carpetas distintas para no pisarse en el mismo PC."""

from __future__ import annotations

import os
import sys
from pathlib import Path


def is_frozen() -> bool:
    return bool(getattr(sys, "frozen", False))


def _f(frozen: bool | None) -> bool:
    return is_frozen() if frozen is None else frozen


def package_dir() -> Path:
    """Carpeta del paquete calltx (dentro del ejecutable si está empaquetado)."""
    if is_frozen():
        return Path(getattr(sys, "_MEIPASS", Path(sys.executable).parent)) / "calltx"
    return Path(__file__).resolve().parent


def web_dir() -> Path:
    return package_dir() / "ui" / "web"


def assets_dir() -> Path:
    return package_dir() / "ui" / "assets"


def _appdata() -> Path:
    return Path(os.environ.get("APPDATA") or Path.home() / "AppData" / "Roaming")


def _localappdata() -> Path:
    return Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local")


def data_dir(frozen: bool | None = None) -> Path:
    return _appdata() / ("CallTranscriber" if _f(frozen) else "CallTranscriber-dev")


def default_output_dir(frozen: bool | None = None) -> Path:
    return Path.home() / "Documents" / ("Llamadas" if _f(frozen) else "Llamadas (dev)")


def cuda_dir() -> Path:
    return _localappdata() / "Transcriptor" / "cuda"


def instance_names(frozen: bool | None = None) -> tuple[str, str]:
    """(mutex de instancia única, evento para mostrar la ventana)."""
    if _f(frozen):
        return "CallTranscriberMutex", "CallTranscriberShow"
    return "CallTranscriberDevMutex", "CallTranscriberDevShow"


def http_port(frozen: bool | None = None) -> int:
    """Puerto local donde pywebview sirve la página. La instalada usa el de siempre (42001: así
    conserva sus datos de navegador); la de desarrollo, otro, para no cargar los archivos de la
    instalada cuando ambas están abiertas."""
    return 42001 if _f(frozen) else 42011

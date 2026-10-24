"""Botones de la barra propia (minimizar, maximizar/restaurar, cerrar a la bandeja) y aspecto
de la ventana (esquinas redondeadas, modo oscuro). Mover y redimensionar los hace Windows:
ver nativeframe.py y la zona `app-region: drag` de la página."""

from __future__ import annotations

import ctypes
import logging
from ctypes import wintypes
from typing import Callable

log = logging.getLogger(__name__)

SW_MAXIMIZE = 3
SW_MINIMIZE = 6
SW_RESTORE = 9
DWMWA_USE_IMMERSIVE_DARK_MODE = 20
DWMWA_WINDOW_CORNER_PREFERENCE = 33
DWMWCP_ROUND = 2


class MARGINS(ctypes.Structure):
    _fields_ = [("l", ctypes.c_int), ("r", ctypes.c_int), ("t", ctypes.c_int), ("b", ctypes.c_int)]


_u = ctypes.WinDLL("user32", use_last_error=True)
_u.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
_u.IsZoomed.argtypes = [wintypes.HWND]


class Win32:
    def minimize(self, hwnd) -> None:
        _u.ShowWindow(hwnd, SW_MINIMIZE)

    def maximize(self, hwnd) -> None:
        _u.ShowWindow(hwnd, SW_MAXIMIZE)

    def restore(self, hwnd) -> None:
        _u.ShowWindow(hwnd, SW_RESTORE)

    def is_zoomed(self, hwnd) -> bool:
        return bool(_u.IsZoomed(hwnd))


class Chrome:
    """Botones de la barra propia. Maximizar es el de Windows (los límites al área de trabajo
    los pone la ventana), así encajar, doble clic, arrastrar para restaurar y Win+↓ son nativos."""

    def __init__(self, get_hwnd: Callable[[], int | None], win32=None, hide: Callable[[], None] = lambda: None):
        self._hwnd = get_hwnd
        self._w = win32 or Win32()
        self._hide = hide

    @property
    def maximized(self) -> bool:
        h = self._hwnd()
        return bool(h and self._w.is_zoomed(h))

    def minimize(self) -> None:
        if h := self._hwnd():
            self._w.minimize(h)

    def hide(self) -> None:
        self._hide()

    def toggle_maximize(self) -> bool:
        h = self._hwnd()
        if not h:
            return False
        if self._w.is_zoomed(h):
            self._w.restore(h)
            return False
        self._w.maximize(h)
        return True


def apply_chrome(hwnd: int) -> None:
    """Esquinas redondeadas, modo oscuro y sombra (los estilos de ventana los pone nativeframe)."""
    try:
        dwm = ctypes.WinDLL("dwmapi")
        val = ctypes.c_int(DWMWCP_ROUND)
        dwm.DwmSetWindowAttribute(wintypes.HWND(hwnd), DWMWA_WINDOW_CORNER_PREFERENCE, ctypes.byref(val), 4)
        dark = ctypes.c_int(1)
        dwm.DwmSetWindowAttribute(wintypes.HWND(hwnd), DWMWA_USE_IMMERSIVE_DARK_MODE, ctypes.byref(dark), 4)
        m = MARGINS(0, 0, 1, 0)
        dwm.DwmExtendFrameIntoClientArea(wintypes.HWND(hwnd), ctypes.byref(m))
    except OSError:
        log.debug("DWM no disponible", exc_info=True)

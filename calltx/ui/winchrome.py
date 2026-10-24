"""Comportamiento de ventana para la barra propia (ventana sin marco de Windows).

Mover y redimensionar se delegan en Windows (WM_NCLBUTTONDOWN), así el sistema hace el
arrastre real (con encaje a los bordes). Maximizar ajusta la ventana al área de trabajo del
monitor en el que está, sin tapar la barra de tareas.
"""

from __future__ import annotations

import ctypes
import logging
from ctypes import wintypes
from typing import Callable

log = logging.getLogger(__name__)

HTCAPTION = 2
EDGES = {"left": 10, "right": 11, "top": 12, "topleft": 13, "topright": 14,
         "bottom": 15, "bottomleft": 16, "bottomright": 17}
WM_NCLBUTTONDOWN = 0x00A1
WM_SYSCOMMAND = 0x0112
# SC_MOVE / SC_SIZE + código de borde (WMSZ_*), forma clásica de arrancar el arrastre nativo
SC_FOR_HIT = {2: 0xF012, 10: 0xF001, 11: 0xF002, 12: 0xF003, 13: 0xF004, 14: 0xF005,
              15: 0xF006, 16: 0xF007, 17: 0xF008}
MONITOR_DEFAULTTONEAREST = 2
SW_MINIMIZE = 6
GWL_STYLE = -16
WS_THICKFRAME = 0x00040000
WS_MINIMIZEBOX = 0x00020000
WS_MAXIMIZEBOX = 0x00010000
WS_SYSMENU = 0x00080000
DWMWA_USE_IMMERSIVE_DARK_MODE = 20
DWMWA_WINDOW_CORNER_PREFERENCE = 33
DWMWCP_ROUND = 2

Rect = tuple[int, int, int, int]


class MONITORINFO(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT),
                ("rcWork", wintypes.RECT), ("dwFlags", wintypes.DWORD)]


class MARGINS(ctypes.Structure):
    _fields_ = [("l", ctypes.c_int), ("r", ctypes.c_int), ("t", ctypes.c_int), ("b", ctypes.c_int)]


_u = ctypes.WinDLL("user32", use_last_error=True)
_u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
_u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
_u.MonitorFromWindow.argtypes = [wintypes.HWND, wintypes.DWORD]
_u.MonitorFromWindow.restype = wintypes.HANDLE
_u.GetMonitorInfoW.argtypes = [wintypes.HANDLE, ctypes.POINTER(MONITORINFO)]
_u.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
_u.SendMessageW.restype = ctypes.c_ssize_t
_u.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
_u.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
_u.IsZoomed.argtypes = [wintypes.HWND]
_u.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
_u.FindWindowW.restype = wintypes.HWND
_u.GetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int]
_u.GetWindowLongPtrW.restype = ctypes.c_ssize_t
_u.SetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int, ctypes.c_ssize_t]
_u.SetWindowLongPtrW.restype = ctypes.c_ssize_t
_u.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
_u.IsWindowVisible.argtypes = [wintypes.HWND]
_u.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]


class Win32:
    def get_rect(self, hwnd) -> Rect:
        r = wintypes.RECT()
        _u.GetWindowRect(hwnd, ctypes.byref(r))
        return r.left, r.top, r.right, r.bottom

    def set_rect(self, hwnd, rect: Rect) -> None:
        l, t, r, b = rect
        _u.SetWindowPos(hwnd, None, l, t, r - l, b - t, 0x0004 | 0x0010)  # NOZORDER | NOACTIVATE

    def work_area(self, hwnd) -> Rect:
        mi = MONITORINFO()
        mi.cbSize = ctypes.sizeof(MONITORINFO)
        _u.GetMonitorInfoW(_u.MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), ctypes.byref(mi))
        w = mi.rcWork
        return w.left, w.top, w.right, w.bottom

    def send_nc(self, hwnd, hit: int) -> None:
        # Se publica en la cola de la ventana: el hilo de la interfaz arranca el bucle nativo
        # de mover/redimensionar (con encaje a los bordes) mientras el botón sigue pulsado.
        _u.PostMessageW(hwnd, WM_SYSCOMMAND, SC_FOR_HIT[hit], 0)

    def minimize(self, hwnd) -> None:
        _u.ShowWindow(hwnd, SW_MINIMIZE)

    def maximize(self, hwnd) -> None:
        _u.ShowWindow(hwnd, 3)  # SW_MAXIMIZE

    def restore(self, hwnd) -> None:
        _u.ShowWindow(hwnd, 9)  # SW_RESTORE

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


def find_window(title: str, pid: int) -> int | None:
    found: list[int] = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, _):
        buf = ctypes.create_unicode_buffer(256)
        _u.GetWindowTextW(h, buf, 256)
        if buf.value == title:
            p = wintypes.DWORD()
            _u.GetWindowThreadProcessId(h, ctypes.byref(p))
            if p.value == pid:
                found.append(h)
                return False
        return True

    _u.EnumWindows(cb, 0)
    return found[0] if found else None


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

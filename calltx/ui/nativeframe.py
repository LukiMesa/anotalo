"""Marco nativo para la ventana sin barra de Windows.

La ventana recupera los estilos normales de Windows (título, borde redimensionable, minimizar,
maximizar) para que el propio Windows anime minimizar, maximizar, mostrar y ocultar. El área
que Windows dibujaría como marco se elimina (WM_NCCALCSIZE) y el borde fino que deja la
ventana alrededor de la página se convierte en bordes de redimensionado nativos (WM_NCHITTEST).
"""

from __future__ import annotations

from typing import Callable

Rect = tuple[int, int, int, int]  # left, top, right, bottom

WS_CAPTION = 0x00C00000
WS_THICKFRAME = 0x00040000
WS_SYSMENU = 0x00080000
WS_MINIMIZEBOX = 0x00020000
WS_MAXIMIZEBOX = 0x00010000
FRAME_BITS = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX

SM_CXSIZEFRAME = 32
SM_CYSIZEFRAME = 33
SM_CXPADDEDBORDER = 92

HTCLIENT = 1
HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT = range(10, 18)


def keep_frame_style(style: int) -> int:
    """El estilo con los bits de marco que Windows necesita para animar la ventana."""
    return style | FRAME_BITS


def frame_inset(dpi: int, metric_for_dpi: Callable[[int, int], int]) -> tuple[int, int]:
    """Grosor del marco que Windows deja fuera de la pantalla al maximizar, para este DPI."""
    pad = metric_for_dpi(SM_CXPADDEDBORDER, dpi)
    return metric_for_dpi(SM_CXSIZEFRAME, dpi) + pad, metric_for_dpi(SM_CYSIZEFRAME, dpi) + pad


def client_rect(proposed: Rect, maximized: bool, inset: tuple[int, int]) -> Rect:
    """Zona de contenido: toda la ventana (sin marco); maximizada, sin lo que sobresale."""
    if not maximized:
        return proposed
    l, t, r, b = proposed
    ix, iy = inset
    return l + ix, t + iy, r - ix, b - iy


def hit_test(x: int, y: int, w: int, h: int, border: int, maximized: bool) -> int:
    """Qué parte de la ventana hay en (x, y), relativo a su esquina: borde o contenido."""
    if maximized:
        return HTCLIENT
    c = border * 3
    top, bottom = y < c, y >= h - c
    left, right = x < c, x >= w - c
    if top and left:
        return HTTOPLEFT
    if top and right:
        return HTTOPRIGHT
    if bottom and left:
        return HTBOTTOMLEFT
    if bottom and right:
        return HTBOTTOMRIGHT
    if x < border:
        return HTLEFT
    if x >= w - border:
        return HTRIGHT
    if y < border:
        return HTTOP
    if y >= h - border:
        return HTBOTTOM
    return HTCLIENT


# --- Windows -----------------------------------------------------------------
import ctypes  # noqa: E402
import logging  # noqa: E402
from ctypes import wintypes  # noqa: E402

log = logging.getLogger(__name__)

WM_STYLECHANGING = 0x007C
WM_NCDESTROY = 0x0082
WM_NCCALCSIZE = 0x0083
WM_NCHITTEST = 0x0084
GWL_STYLE = -16
SWP_FRAMECHANGED = 0x0020
_SWP_KEEP = 0x0001 | 0x0002 | 0x0004 | 0x0010  # NOSIZE | NOMOVE | NOZORDER | NOACTIVATE
_SUBCLASS_ID = 0xCA11

LRESULT = ctypes.c_ssize_t
SUBCLASSPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT, wintypes.WPARAM,
                                  wintypes.LPARAM, ctypes.c_size_t, ctypes.c_size_t)


class NCCALCSIZE_PARAMS(ctypes.Structure):
    _fields_ = [("rgrc", wintypes.RECT * 3), ("lppos", ctypes.c_void_p)]


class STYLESTRUCT(ctypes.Structure):
    _fields_ = [("styleOld", wintypes.DWORD), ("styleNew", wintypes.DWORD)]


_u = ctypes.WinDLL("user32", use_last_error=True)
_c = ctypes.WinDLL("comctl32")
_c.SetWindowSubclass.argtypes = [wintypes.HWND, SUBCLASSPROC, ctypes.c_size_t, ctypes.c_size_t]
_c.SetWindowSubclass.restype = wintypes.BOOL
_c.RemoveWindowSubclass.argtypes = [wintypes.HWND, SUBCLASSPROC, ctypes.c_size_t]
_c.RemoveWindowSubclass.restype = wintypes.BOOL
_c.DefSubclassProc.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
_c.DefSubclassProc.restype = LRESULT
_u.GetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int]
_u.GetWindowLongPtrW.restype = ctypes.c_ssize_t
_u.SetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int, ctypes.c_ssize_t]
_u.SetWindowLongPtrW.restype = ctypes.c_ssize_t
_u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                            ctypes.c_int, wintypes.UINT]
_u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
_u.IsZoomed.argtypes = [wintypes.HWND]
_u.GetDpiForWindow.argtypes = [wintypes.HWND]
_u.GetDpiForWindow.restype = wintypes.UINT
_u.GetSystemMetricsForDpi.argtypes = [ctypes.c_int, wintypes.UINT]

_procs: dict[int, SUBCLASSPROC] = {}  # el callback debe vivir mientras exista la ventana


def _signed16(v: int) -> int:
    v &= 0xFFFF
    return v - 0x10000 if v & 0x8000 else v


def _make_proc(border: int) -> SUBCLASSPROC:
    def proc(hwnd, msg, wp, lp, _id, _data):
        try:
            if msg == WM_NCCALCSIZE:
                if wp:
                    p = NCCALCSIZE_PARAMS.from_address(lp)
                    r = p.rgrc[0]
                    dpi = _u.GetDpiForWindow(hwnd) or 96
                    l, t, rr, b = client_rect((r.left, r.top, r.right, r.bottom), bool(_u.IsZoomed(hwnd)),
                                              frame_inset(dpi, _u.GetSystemMetricsForDpi))
                    r.left, r.top, r.right, r.bottom = l, t, rr, b
                return 0  # sin marco: toda la ventana es contenido
            if msg == WM_NCHITTEST:
                wr = wintypes.RECT()
                _u.GetWindowRect(hwnd, ctypes.byref(wr))
                hit = hit_test(_signed16(lp) - wr.left, _signed16(lp >> 16) - wr.top,
                               wr.right - wr.left, wr.bottom - wr.top, border, bool(_u.IsZoomed(hwnd)))
                if hit != HTCLIENT:
                    return hit
            elif msg == WM_STYLECHANGING and (wp & 0xFFFFFFFF) == (GWL_STYLE & 0xFFFFFFFF):
                ss = STYLESTRUCT.from_address(lp)
                ss.styleNew = keep_frame_style(ss.styleNew) & 0xFFFFFFFF
            elif msg == WM_NCDESTROY:
                cb = _procs.pop(int(hwnd), None)
                if cb is not None:
                    _c.RemoveWindowSubclass(hwnd, cb, _SUBCLASS_ID)
        except Exception:  # noqa: BLE001 - nunca propagar a Windows
            log.exception("Marco nativo: error en el mensaje 0x%04X", msg)
        return _c.DefSubclassProc(hwnd, msg, wp, lp)

    return SUBCLASSPROC(proc)


def install(hwnd: int, border: int) -> None:
    """Llamar en el hilo de la ventana. Primero el subclass, luego los estilos: así nunca se
    ve el marco de Windows."""
    if hwnd in _procs:
        return
    cb = _make_proc(border)
    if not _c.SetWindowSubclass(hwnd, cb, _SUBCLASS_ID, 0):
        raise OSError("SetWindowSubclass falló")
    _procs[hwnd] = cb
    style = _u.GetWindowLongPtrW(hwnd, GWL_STYLE)
    _u.SetWindowLongPtrW(hwnd, GWL_STYLE, keep_frame_style(style))
    _u.SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | _SWP_KEEP)

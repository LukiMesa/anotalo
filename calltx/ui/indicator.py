"""Indicador de grabación: píldora pequeña abajo al centro, siempre encima y sin robar el foco.

Se dibuja con Pillow (a 3× y reducido, para bordes suaves) y se muestra en una ventana
Win32 "layered" con transparencia por píxel, que ignora el ratón y nunca se activa.
"""

from __future__ import annotations

import ctypes
import logging
import queue
import threading
import time
from collections import deque
from ctypes import wintypes
from pathlib import Path
from typing import Literal

from PIL import Image, ImageChops, ImageDraw, ImageFont

log = logging.getLogger(__name__)

PillState = Literal["recording", "transcribing", "copied", "saved"]

# Tokens del tema Noche
BG = (16, 20, 31, 240)  # --surface-sunken al 94 %
BORDER = (46, 56, 80, 255)  # --line
INK = (238, 243, 247, 255)  # --ink
ORBITA = (46, 197, 230, 255)  # --orbita
ERROR = (255, 122, 122, 255)  # --error
EXITO = (61, 220, 151, 255)  # --exito
LINE_STRONG = (90, 104, 131, 255)  # --line-strong

SS = 3  # supermuestreo
FONT_PATH = Path(__file__).parent / "web" / "fonts" / "JetBrainsMono.ttf"
TEXTS = {"transcribing": "Transcribiendo…", "copied": "Copiado al portapapeles", "saved": "Transcripción guardada"}

_font_cache: dict[int, ImageFont.FreeTypeFont] = {}


def _font(px: int) -> ImageFont.FreeTypeFont:
    if px not in _font_cache:
        try:
            f = ImageFont.truetype(str(FONT_PATH), px)
            try:
                f.set_variation_by_axes([500])
            except Exception:  # noqa: BLE001 - fuente no variable
                pass
        except OSError:
            f = ImageFont.truetype("consola.ttf", px)
        _font_cache[px] = f
    return _font_cache[px]


def _fmt(elapsed_s: float) -> str:
    s = max(0, int(elapsed_s))
    return f"{s // 60:02d}:{s % 60:02d}" if s < 3600 else f"{s // 3600}:{s % 3600 // 60:02d}:{s % 60:02d}"


def render_pill(state: PillState, levels: list[float], elapsed_s: float, scale: float = 1.0, spin: float = 0.0) -> Image.Image:
    k = scale * SS
    h = round(32 * k)
    pad_l, pad_r, gap = round(12 * k), round(14 * k), round(9 * k)
    font = _font(max(8, round(11.5 * k)))
    text = _fmt(elapsed_s) if state == "recording" else TEXTS[state]
    tw = round(font.getlength(text))
    icon_w = round(8 * k) if state == "recording" else round(14 * k)
    bars_w = round((6 * 3 + 5 * 2) * k) if state == "recording" else 0
    w = pad_l + icon_w + gap + (bars_w + gap if bars_w else 0) + tw + pad_r

    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((0, 0, w - 1, h - 1), radius=h // 2, fill=BG, outline=BORDER, width=max(1, round(k)))
    cy = h / 2
    x = pad_l
    if state == "recording":
        r = 4 * k
        halo = 7 * k
        d.ellipse((x + r - halo, cy - halo, x + r + halo, cy + halo), fill=(255, 122, 122, 50))
        d.ellipse((x, cy - r, x + 2 * r, cy + r), fill=ERROR)
        x += icon_w + gap
        vals = (list(levels) + [0.0] * 6)[:6]
        bw, bg_ = 3 * k, 2 * k
        for i, v in enumerate(vals):
            bh = (3 + 11 * max(0.0, min(1.0, v))) * k
            bx = x + i * (bw + bg_)
            d.rounded_rectangle((bx, cy - bh / 2, bx + bw, cy + bh / 2), radius=bw / 2, fill=ORBITA)
        x += bars_w + gap
    elif state == "transcribing":
        s = 12 * k
        box = (x + 1 * k, cy - s / 2, x + 1 * k + s, cy + s / 2)
        d.ellipse(box, outline=LINE_STRONG, width=round(2 * k))
        a0 = (spin * 360) % 360
        d.arc(box, a0, a0 + 90, fill=ORBITA, width=round(2 * k))
        x += icon_w + gap
    else:
        pts = [(x + 1 * k, cy), (x + 5.5 * k, cy + 4.5 * k), (x + 13 * k, cy - 5 * k)]
        d.line(pts, fill=EXITO, width=round(2 * k), joint="curve")
        x += icon_w + gap
    bbox = font.getbbox(text)
    d.text((x, cy - (bbox[1] + bbox[3]) / 2), text, font=font, fill=INK)
    return img.resize((max(1, round(w / SS)), max(1, round(h / SS))), Image.LANCZOS)


# --- ventana Win32 -----------------------------------------------------------

WS_POPUP = 0x80000000
WS_EX_LAYERED = 0x00080000
WS_EX_TRANSPARENT = 0x00000020
WS_EX_TOPMOST = 0x00000008
WS_EX_TOOLWINDOW = 0x00000080
WS_EX_NOACTIVATE = 0x08000000
SW_SHOWNOACTIVATE = 4
SW_HIDE = 0
ULW_ALPHA = 2
AC_SRC_ALPHA = 1
SPI_GETWORKAREA = 0x0030
PM_REMOVE = 1
QS_ALLINPUT = 0x04FF
HWND_TOPMOST = -1
SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE = 0x10, 0x2, 0x1

WNDPROC = ctypes.WINFUNCTYPE(ctypes.c_ssize_t, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)


class WNDCLASSW(ctypes.Structure):
    _fields_ = [("style", wintypes.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", ctypes.c_int),
                ("cbWndExtra", ctypes.c_int), ("hInstance", wintypes.HINSTANCE), ("hIcon", wintypes.HICON),
                ("hCursor", wintypes.HANDLE), ("hbrBackground", wintypes.HBRUSH),
                ("lpszMenuName", wintypes.LPCWSTR), ("lpszClassName", wintypes.LPCWSTR)]


class BLENDFUNCTION(ctypes.Structure):
    _fields_ = [("BlendOp", ctypes.c_ubyte), ("BlendFlags", ctypes.c_ubyte),
                ("SourceConstantAlpha", ctypes.c_ubyte), ("AlphaFormat", ctypes.c_ubyte)]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [("biSize", wintypes.DWORD), ("biWidth", wintypes.LONG), ("biHeight", wintypes.LONG),
                ("biPlanes", wintypes.WORD), ("biBitCount", wintypes.WORD), ("biCompression", wintypes.DWORD),
                ("biSizeImage", wintypes.DWORD), ("biXPelsPerMeter", wintypes.LONG),
                ("biYPelsPerMeter", wintypes.LONG), ("biClrUsed", wintypes.DWORD), ("biClrImportant", wintypes.DWORD)]


_user32 = ctypes.WinDLL("user32", use_last_error=True)
_gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
_user32.DefWindowProcW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
_user32.DefWindowProcW.restype = ctypes.c_ssize_t
_user32.CreateWindowExW.restype = wintypes.HWND
_user32.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
                                    ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                    wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID]
_user32.GetDC.restype = wintypes.HDC
_user32.GetDC.argtypes = [wintypes.HWND]
_gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
_gdi32.CreateCompatibleDC.restype = wintypes.HDC
_gdi32.CreateDIBSection.restype = wintypes.HBITMAP
_gdi32.CreateDIBSection.argtypes = [wintypes.HDC, ctypes.c_void_p, wintypes.UINT, ctypes.POINTER(ctypes.c_void_p),
                                    wintypes.HANDLE, wintypes.DWORD]
_gdi32.SelectObject.restype = wintypes.HGDIOBJ
_gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
_gdi32.DeleteObject.argtypes = [wintypes.HGDIOBJ]
_gdi32.DeleteDC.argtypes = [wintypes.HDC]
_user32.UpdateLayeredWindow.argtypes = [wintypes.HWND, wintypes.HDC, ctypes.POINTER(wintypes.POINT),
                                        ctypes.POINTER(wintypes.SIZE), wintypes.HDC, ctypes.POINTER(wintypes.POINT),
                                        wintypes.COLORREF, ctypes.POINTER(BLENDFUNCTION), wintypes.DWORD]
_user32.ReleaseDC.argtypes = [wintypes.HWND, wintypes.HDC]
_user32.RegisterClassW.argtypes = [ctypes.POINTER(WNDCLASSW)]
_kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
_kernel32.GetModuleHandleW.restype = wintypes.HMODULE
_kernel32.GetModuleHandleW.argtypes = [wintypes.LPCWSTR]
_user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
_user32.DestroyWindow.argtypes = [wintypes.HWND]
_user32.GetDpiForWindow.argtypes = [wintypes.HWND]
_user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                 ctypes.c_int, wintypes.UINT]


@WNDPROC
def _wndproc(hwnd, msg, wp, lp):
    return _user32.DefWindowProcW(hwnd, msg, wp, lp)


class Indicator:
    FPS = 30
    DONE_S = 1.5
    FADE_S = 0.18

    def __init__(self):
        self._cmds: queue.Queue = queue.Queue()
        self._levels: deque[float] = deque([0.0] * 6, maxlen=6)
        self._shown = [0.0] * 6
        self._thread = threading.Thread(target=self._run, name="indicator", daemon=True)
        self._thread.start()

    # API (cualquier hilo)
    def show_recording(self) -> None:
        self._cmds.put(("recording",))

    def push_level(self, rms: float) -> None:
        self._levels.append(min(1.0, rms * 12))

    def show_transcribing(self) -> None:
        self._cmds.put(("transcribing",))

    def show_done(self, copied: bool) -> None:
        self._cmds.put(("copied" if copied else "saved",))

    def hide(self) -> None:
        self._cmds.put(("hide",))

    def close(self) -> None:
        self._cmds.put(("quit",))
        self._thread.join(2)

    # hilo de la ventana
    def _run(self) -> None:
        try:
            _user32.SetThreadDpiAwarenessContext(ctypes.c_void_p(-4))  # per-monitor v2
        except Exception:  # noqa: BLE001
            pass
        hinst = _kernel32.GetModuleHandleW(None)
        wc = WNDCLASSW()
        wc.lpfnWndProc = _wndproc
        wc.hInstance = hinst
        wc.lpszClassName = "CallTranscriberIndicator"
        _user32.RegisterClassW(ctypes.byref(wc))
        ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
        self._hwnd = _user32.CreateWindowExW(ex, wc.lpszClassName, "Grabando", WS_POPUP, 0, 0, 1, 1,
                                             None, None, hinst, None)
        try:
            dpi = _user32.GetDpiForWindow(self._hwnd) or 96
        except Exception:  # noqa: BLE001
            dpi = 96
        self._scale = dpi / 96
        state: str | None = None
        started = 0.0
        done_at = 0.0
        visible = False
        msg = wintypes.MSG()
        last = time.monotonic()
        while True:
            _user32.MsgWaitForMultipleObjects(0, None, False, int(1000 / self.FPS), QS_ALLINPUT)
            while _user32.PeekMessageW(ctypes.byref(msg), None, 0, 0, PM_REMOVE):
                _user32.TranslateMessage(ctypes.byref(msg))
                _user32.DispatchMessageW(ctypes.byref(msg))
            now = time.monotonic()
            dt, last = now - last, now
            try:
                while True:
                    cmd = self._cmds.get_nowait()[0]
                    if cmd == "quit":
                        _user32.DestroyWindow(self._hwnd)
                        return
                    if cmd == "hide":
                        state = None
                    else:
                        if cmd == "recording":
                            started = now
                            self._levels.extend([0.0] * 6)
                        if cmd in ("copied", "saved"):
                            done_at = now
                        state = cmd
            except queue.Empty:
                pass
            alpha = 255
            if state in ("copied", "saved"):
                t = now - done_at
                if t > self.DONE_S + self.FADE_S:
                    state = None
                elif t > self.DONE_S:
                    alpha = int(255 * (1 - (t - self.DONE_S) / self.FADE_S))
            if state is None:
                if visible:
                    _user32.ShowWindow(self._hwnd, SW_HIDE)
                    visible = False
                continue
            # barras: suben rápido, bajan en ~120 ms
            target = list(self._levels)
            self._shown = [max(t, s - dt / 0.12) for t, s in zip(target, self._shown)]
            try:
                img = render_pill(state, self._shown, now - started, self._scale, spin=now * 1.2)
                self._blit(img, alpha)
            except Exception:  # noqa: BLE001 - un fotograma fallido no apaga el indicador
                log.exception("Error dibujando el indicador")
                continue
            if not visible:
                _user32.ShowWindow(self._hwnd, SW_SHOWNOACTIVATE)
                _user32.SetWindowPos(self._hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE)
                visible = True

    def _blit(self, img: Image.Image, alpha: int) -> None:
        w, h = img.size
        rect = wintypes.RECT()
        _user32.SystemParametersInfoW(SPI_GETWORKAREA, 0, ctypes.byref(rect), 0)
        x = (rect.left + rect.right - w) // 2
        y = rect.bottom - h - round(16 * self._scale)
        # BGRA premultiplicado, de arriba abajo
        r, g, b, a = img.split()
        premul = Image.merge("RGBA", [ImageChops.multiply(c, a) for c in (b, g, r)] + [a])
        data = premul.tobytes()
        screen = _user32.GetDC(None)
        mem = _gdi32.CreateCompatibleDC(screen)
        bmi = BITMAPINFOHEADER(ctypes.sizeof(BITMAPINFOHEADER), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
        bits = ctypes.c_void_p()
        hbmp = _gdi32.CreateDIBSection(mem, ctypes.byref(bmi), 0, ctypes.byref(bits), None, 0)
        if not hbmp or not bits.value:
            _gdi32.DeleteDC(mem)
            _user32.ReleaseDC(None, screen)
            raise OSError("CreateDIBSection falló")
        ctypes.memmove(bits, data, len(data))
        old = _gdi32.SelectObject(mem, hbmp)
        blend = BLENDFUNCTION(0, 0, alpha, AC_SRC_ALPHA)
        _user32.UpdateLayeredWindow(self._hwnd, screen, ctypes.byref(wintypes.POINT(x, y)),
                                    ctypes.byref(wintypes.SIZE(w, h)), mem, ctypes.byref(wintypes.POINT(0, 0)),
                                    0, ctypes.byref(blend), ULW_ALPHA)
        _gdi32.SelectObject(mem, old)
        _gdi32.DeleteObject(hbmp)
        _gdi32.DeleteDC(mem)
        _user32.ReleaseDC(None, screen)


def wire_indicator(app, settings, indicator) -> None:
    """Conecta la app con el indicador. Al terminar una grabación la píldora siempre se cierra,
    aunque se haya desactivado el indicador a mitad de grabación."""
    state = {"shown": False}

    def on_start(label):
        if settings.show_indicator:
            indicator.show_recording()
            state["shown"] = True

    def on_stopping(label):
        if state["shown"] and settings.show_indicator:
            indicator.show_transcribing()

    def on_end(path, label, copied):
        if state["shown"] and settings.show_indicator and path is not None:
            indicator.show_done(copied)
        else:
            indicator.hide()
        state["shown"] = False

    def on_toggle(value):
        if not value:
            indicator.hide()
            state["shown"] = False

    app.on_session_start.append(on_start)
    app.on_level.append(indicator.push_level)
    app.on_session_stopping.append(on_stopping)
    app.on_session_end.append(on_end)
    app.after_apply["show_indicator"] = on_toggle

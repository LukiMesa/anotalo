"""Atajo de teclado global con RegisterHotKey de Windows."""

from __future__ import annotations

import ctypes
import logging
import threading
from ctypes import wintypes
from typing import Callable

log = logging.getLogger(__name__)

MOD_ALT = 0x0001
MOD_CONTROL = 0x0002
MOD_SHIFT = 0x0004
MOD_WIN = 0x0008
MOD_NOREPEAT = 0x4000
WM_HOTKEY = 0x0312
WM_QUIT = 0x0012

_MODS = {"<ctrl>": MOD_CONTROL, "<alt>": MOD_ALT, "<shift>": MOD_SHIFT, "<cmd>": MOD_WIN, "<win>": MOD_WIN}


def parse_hotkey(spec: str) -> tuple[int, int]:
    """'<ctrl>+<alt>+r' → (modificadores, código de tecla virtual)."""
    mods, vk = MOD_NOREPEAT, None
    for part in (p.strip().lower() for p in spec.split("+")):
        if part in _MODS:
            mods |= _MODS[part]
        elif len(part) == 1 and part.isalnum():
            vk = ord(part.upper())
        elif part.startswith("<f") and part.endswith(">") and part[2:-1].isdigit() and 1 <= int(part[2:-1]) <= 24:
            vk = 0x70 + int(part[2:-1]) - 1
        else:
            raise ValueError(f"Tecla no reconocida en el atajo: {part!r}")
    if vk is None:
        raise ValueError(f"El atajo {spec!r} no tiene tecla principal")
    return mods, vk


class HotkeyError(Exception):
    pass


def validate_hotkey(spec: str) -> tuple[int, int]:
    """Como parse_hotkey, pero exige Ctrl, Alt o Win (Shift solo no basta)."""
    mods, vk = parse_hotkey(spec)
    if not mods & (MOD_CONTROL | MOD_ALT | MOD_WIN):
        raise ValueError("El atajo necesita Ctrl, Alt o Win")
    return mods, vk


WM_APP_REBIND = 0x8000 + 1


class GlobalHotkey:
    def __init__(self, spec: str, callback: Callable[[], None]):
        self._mods, self._vk = parse_hotkey(spec)
        self.spec = spec
        self._cb = callback
        self._tid: int | None = None
        self._thread: threading.Thread | None = None
        self._started = threading.Event()
        self.registered = threading.Event()
        self._rebind_req: tuple[str, int, int] | None = None
        self._rebind_done = threading.Event()
        self._rebind_ok = False

    def start(self) -> None:
        self._thread = threading.Thread(target=self._run, name="hotkey", daemon=True)
        self._thread.start()
        self._started.wait(2)

    def rebind(self, spec: str) -> None:
        """Cambia el atajo en vivo. Si el nuevo no se puede registrar, se queda el anterior."""
        mods, vk = validate_hotkey(spec)
        if self._tid is None:
            raise HotkeyError("El atajo no está activo")
        self._rebind_req = (spec, mods, vk)
        self._rebind_done.clear()
        ctypes.windll.user32.PostThreadMessageW(self._tid, WM_APP_REBIND, 0, 0)
        if not self._rebind_done.wait(2):
            raise HotkeyError("No se pudo cambiar el atajo")
        if not self._rebind_ok:
            raise HotkeyError("Esa combinación ya la usa otra aplicación")

    def _run(self) -> None:
        user32 = ctypes.windll.user32
        msg = wintypes.MSG()
        # Crea la cola de mensajes del hilo antes de anunciar que está listo.
        user32.PeekMessageW(ctypes.byref(msg), None, 0, 0, 0)
        self._tid = ctypes.windll.kernel32.GetCurrentThreadId()
        if user32.RegisterHotKey(None, 1, self._mods, self._vk):
            self.registered.set()
        else:
            log.error("No se pudo registrar el atajo %s (¿lo usa otra app?)", self.spec)
        self._started.set()
        try:
            while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
                if msg.message == WM_HOTKEY:
                    try:
                        self._cb()
                    except Exception:  # noqa: BLE001
                        log.exception("Error en la acción del atajo")
                elif msg.message == WM_APP_REBIND and self._rebind_req:
                    spec, mods, vk = self._rebind_req
                    was = self.registered.is_set()
                    user32.UnregisterHotKey(None, 1)
                    if user32.RegisterHotKey(None, 1, mods, vk):
                        self._mods, self._vk, self.spec = mods, vk, spec
                        self.registered.set()
                        self._rebind_ok = True
                    else:
                        if was:
                            user32.RegisterHotKey(None, 1, self._mods, self._vk)
                        self._rebind_ok = False
                    self._rebind_done.set()
        finally:
            user32.UnregisterHotKey(None, 1)

    def stop(self) -> None:
        if self._tid is not None:
            ctypes.windll.user32.PostThreadMessageW(self._tid, WM_QUIT, 0, 0)
        if self._thread is not None:
            self._thread.join(2)

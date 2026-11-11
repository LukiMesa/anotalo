r"""Detección de llamadas en Teams, Zoom, WhatsApp, Slack, Discord y navegadores.

Windows registra qué apps usan el micrófono en
HKCU\...\CapabilityAccessManager\ConsentStore\microphone: mientras una app lo
usa, su LastUsedTimeStop vale 0. Una llamada empieza cuando una de las apps de
calltx/platforms.py usa el micro unos segundos (y, si hace falta, hay señales de llamada,
p. ej. WhatsApp necesita más tiempo por las notas de voz). Termina SOLO cuando esa app suelta el micro (cambiar
de pestaña o de app no corta la grabación).
"""

from __future__ import annotations

import logging
import threading
import time
from dataclasses import dataclass
from typing import Callable

from .platforms import PLATFORMS, Platform, platform_for_key, site_from_title

log = logging.getLogger(__name__)

CONSENT_KEY = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone"


def _read_consent_store() -> dict[str, tuple[int, int]]:
    import winreg

    out: dict[str, tuple[int, int]] = {}

    def read(root, prefix: str) -> None:
        i = 0
        while True:
            try:
                name = winreg.EnumKey(root, i)
            except OSError:
                return
            i += 1
            with winreg.OpenKey(root, name) as k:
                if name == "NonPackaged" and not prefix:
                    read(k, "NonPackaged/")
                    continue
                try:
                    start = int(winreg.QueryValueEx(k, "LastUsedTimeStart")[0])
                    stop = int(winreg.QueryValueEx(k, "LastUsedTimeStop")[0])
                except OSError:
                    continue
                out[prefix + name] = (start, stop)

    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, CONSENT_KEY) as root:
        read(root, "")
    return out


def mic_in_use(reader: Callable[[], dict[str, tuple[int, int]]] = _read_consent_store) -> set[str]:
    try:
        entries = reader()
    except OSError as e:
        log.debug("No se pudo leer el registro del micrófono: %s", e)
        return set()
    return {k for k, (start, stop) in entries.items() if start > 0 and stop == 0}


@dataclass(frozen=True)
class Detected:
    platform: Platform
    label: str  # "Teams", "Meet", "Chrome"…
    exe: str | None  # proceso cuyo audio es "los demás" (si platform.audio == "process")


def platforms_in_use(keys: set[str]) -> set[str]:
    return {p.key for k in keys if (p := platform_for_key(k)) is not None}


def window_site(platform: Platform) -> str | None:
    """Plataforma reconocible en las ventanas visibles de ese programa (p. ej. "Meet")."""
    import ctypes
    from ctypes import wintypes

    import psutil

    user32 = ctypes.windll.user32
    pids = {p.pid for p in psutil.process_iter(["name"]) if (p.info["name"] or "").lower() in platform.exes}
    if not pids:
        return None
    found: list[str] = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if pid.value not in pids:
            return True
        n = user32.GetWindowTextLengthW(hwnd)
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        if site := site_from_title(buf.value):
            found.append(site)
            return False
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else None


class CallDetector:
    def __init__(
        self,
        on_start: Callable[[Detected], None],
        on_stop: Callable[[Detected], None],
        probe_mic: Callable[[], set[str]] = mic_in_use,
        probe_title: Callable[[Platform], str | None] = window_site,
        clock: Callable[[], float] = time.monotonic,
        start_hold_s: float = 4.0,
        stop_grace_s: float = 10.0,
        enabled: Callable[[Platform], bool] = lambda p: True,
    ):
        self._enabled = enabled
        self._on_start = on_start
        self._on_stop = on_stop
        self._probe_mic = probe_mic
        self._probe_title = probe_title
        self._clock = clock
        self._hold = start_hold_s
        self._grace = stop_grace_s
        self._suspended = False
        self._candidate: Platform | None = None
        self._candidate_since = 0.0
        self.active: Detected | None = None
        self._released_since: float | None = None
        self._dismissed = False
        self._stop_evt = threading.Event()
        self._thread: threading.Thread | None = None

    def suspend(self) -> None:
        self._suspended = True
        self._candidate = None

    def resume(self) -> None:
        self._suspended = False

    def _label(self, platform: Platform) -> str:
        if not platform.browser:
            return platform.name
        try:
            return self._probe_title(platform) or platform.name
        except Exception:  # noqa: BLE001 - el nombre es secundario
            log.debug("No se pudo leer el título de %s", platform.name, exc_info=True)
            return platform.name

    def tick(self) -> None:
        now = self._clock()
        in_use = platforms_in_use(self._probe_mic())
        if self.active is not None:
            if self.active.platform.key in in_use:
                self._released_since = None
            elif self._released_since is None:
                self._released_since = now
            elif now - self._released_since >= self._grace:
                det, self.active, self._released_since = self.active, None, None
                dismissed, self._dismissed = self._dismissed, False
                log.info("Fin de llamada %s", det.label)
                if not dismissed:
                    self._on_stop(det)
            return
        if self._suspended:
            return
        cand = next((p for p in PLATFORMS if p.key in in_use and self._enabled(p)), None)
        if cand is None:
            self._candidate = None
            return
        if cand != self._candidate:
            self._candidate, self._candidate_since = cand, now
            return
        if now - self._candidate_since >= (cand.start_hold_s or self._hold):
            self._candidate = None
            self.active = Detected(cand, self._label(cand), cand.exes[0] if cand.exes else None)
            log.info("Llamada detectada: %s (%s)", self.active.label, cand.name)
            self._on_start(self.active)

    def run(self, poll_s: float = 2.0) -> None:
        self._stop_evt.clear()

        def loop():
            while not self._stop_evt.wait(poll_s):
                try:
                    self.tick()
                except Exception:  # noqa: BLE001
                    log.exception("Error en el detector de llamadas")

        self._thread = threading.Thread(target=loop, name="detector", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop_evt.set()
        if self._thread is not None:
            self._thread.join(5)
            self._thread = None

    def dismiss(self) -> None:
        """La sesión automática se paró a mano: no avisar del fin ni volver a arrancar
        hasta que esa llamada termine de verdad (la app suelte el micro)."""
        self._dismissed = True

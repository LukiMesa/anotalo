"""Detección de llamadas de Teams / Google Meet (Firefox).

Windows registra qué apps usan el micrófono en
HKCU\\...\\CapabilityAccessManager\\ConsentStore\\microphone: mientras una app lo
usa, su LastUsedTimeStop vale 0. Una llamada empieza cuando Teams usa el micro,
o cuando Firefox lo usa y hay una ventana de Meet. Termina SOLO cuando esa app
suelta el micro (cambiar de pestaña o de app no corta la grabación).
"""

from __future__ import annotations

import enum
import logging
import re
import threading
import time
from typing import Callable

log = logging.getLogger(__name__)

CONSENT_KEY = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone"


class CallApp(enum.Enum):
    TEAMS = "Teams"
    MEET = "Meet"


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


def _is_teams(key: str) -> bool:
    k = key.lower()
    return k.startswith("msteams_") or k.endswith("#ms-teams.exe") or k.endswith("#teams.exe")


def _is_firefox(key: str) -> bool:
    return key.lower().endswith("#firefox.exe")


_MEET_TITLE = re.compile(r"^Meet\s*[-–—:]|Google Meet")


def is_meet_title(title: str) -> bool:
    return bool(_MEET_TITLE.search(title))


def firefox_meet_window_open() -> bool:
    import ctypes
    from ctypes import wintypes

    import psutil

    user32 = ctypes.windll.user32
    found = False
    firefox_pids = {p.pid for p in psutil.process_iter(["name"]) if (p.info["name"] or "").lower() == "firefox.exe"}
    if not firefox_pids:
        return False

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _):
        nonlocal found
        if not user32.IsWindowVisible(hwnd):
            return True
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if pid.value not in firefox_pids:
            return True
        n = user32.GetWindowTextLengthW(hwnd)
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        if is_meet_title(buf.value):
            found = True
            return False
        return True

    user32.EnumWindows(cb, 0)
    return found


class CallDetector:
    def __init__(
        self,
        on_start: Callable[[CallApp], None],
        on_stop: Callable[[CallApp], None],
        probe_mic: Callable[[], set[str]] = mic_in_use,
        probe_meet: Callable[[], bool] = firefox_meet_window_open,
        clock: Callable[[], float] = time.monotonic,
        start_hold_s: float = 4.0,
        stop_grace_s: float = 10.0,
        enabled: Callable[[CallApp], bool] = lambda app: True,
    ):
        self._enabled = enabled
        self._on_start = on_start
        self._on_stop = on_stop
        self._probe_mic = probe_mic
        self._probe_meet = probe_meet
        self._clock = clock
        self._hold = start_hold_s
        self._grace = stop_grace_s
        self._suspended = False
        self._candidate: CallApp | None = None
        self._candidate_since = 0.0
        self.active: CallApp | None = None
        self._released_since: float | None = None
        self._dismissed = False
        self._stop_evt = threading.Event()
        self._thread: threading.Thread | None = None

    def suspend(self) -> None:
        self._suspended = True
        self._candidate = None

    def resume(self) -> None:
        self._suspended = False

    def _app_in_use(self, app: CallApp, keys: set[str]) -> bool:
        check = _is_teams if app is CallApp.TEAMS else _is_firefox
        return any(check(k) for k in keys)

    def tick(self) -> None:
        now = self._clock()
        keys = self._probe_mic()
        if self.active is not None:
            if self._app_in_use(self.active, keys):
                self._released_since = None
            elif self._released_since is None:
                self._released_since = now
            elif now - self._released_since >= self._grace:
                app, self.active, self._released_since = self.active, None, None
                dismissed, self._dismissed = self._dismissed, False
                log.info("Fin de llamada %s", app.value)
                if not dismissed:
                    self._on_stop(app)
            return
        if self._suspended:
            return
        cand: CallApp | None = None
        if self._enabled(CallApp.TEAMS) and self._app_in_use(CallApp.TEAMS, keys):
            cand = CallApp.TEAMS
        elif self._enabled(CallApp.MEET) and self._app_in_use(CallApp.MEET, keys) and self._probe_meet():
            cand = CallApp.MEET
        if cand is None:
            self._candidate = None
            return
        if cand != self._candidate:
            self._candidate, self._candidate_since = cand, now
            return
        if now - self._candidate_since >= self._hold:
            self._candidate = None
            self.active = cand
            log.info("Llamada detectada: %s", cand.value)
            self._on_start(cand)

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

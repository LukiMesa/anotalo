"""Coordinación: modo manual (atajo/icono) y modo automático (detector)."""

from __future__ import annotations

import logging
import threading
from pathlib import Path
from typing import Callable, Literal

from .config import Settings
from .detector import CallApp

log = logging.getLogger(__name__)

State = Literal["idle", "armed", "recording"]
EXE_FOR_APP = {CallApp.TEAMS: "ms-teams.exe", CallApp.MEET: "firefox.exe"}


class App:
    def __init__(
        self,
        settings: Settings,
        detector_factory: Callable,
        session_factory: Callable,  # (label, exe|None, on_line) -> session
        notifier: Callable[..., None],
    ):
        self.settings = settings
        self._detector = detector_factory(self.on_call_start, self.on_call_stop)
        self._session_factory = session_factory
        self._notify = notifier
        self._lock = threading.RLock()
        self._session = None
        self._session_auto = False
        self.on_state_change: list[Callable[[State], None]] = []
        self.on_line: list[Callable[[str, str, float, str], None]] = []
        self.on_session_start: list[Callable[[str], None]] = []
        self.last_path: Path | None = None

    # --- estado ------------------------------------------------------------
    @property
    def state(self) -> State:
        if self._session is not None:
            return "recording"
        return "armed" if self.settings.auto_mode else "idle"

    @property
    def session(self):
        return self._session

    def _changed(self) -> None:
        for cb in list(self.on_state_change):
            try:
                cb(self.state)
            except Exception:  # noqa: BLE001
                log.exception("Error en listener de estado")

    def _emit_line(self, *args) -> None:
        for cb in list(self.on_line):
            try:
                cb(*args)
            except Exception:  # noqa: BLE001
                log.exception("Error en listener de líneas")

    def _sync_detector(self) -> None:
        if self.settings.auto_mode and not (self._session is not None and not self._session_auto):
            self._detector.resume()
        else:
            self._detector.suspend()

    # --- ciclo de vida -----------------------------------------------------
    def start(self) -> None:
        self._sync_detector()
        self._detector.run()
        self._changed()

    def quit(self) -> None:
        with self._lock:
            if self._session is not None:
                self._stop_session(user=True)
            self._detector.stop()

    # --- acciones ----------------------------------------------------------
    def toggle(self) -> None:
        with self._lock:
            if self._session is not None:
                self._stop_session(user=True)
            else:
                self.start_manual()

    def start_manual(self) -> None:
        with self._lock:
            if self._session is not None:
                return
            self._begin("Manual", None, auto=False)

    def set_auto(self, enabled: bool) -> None:
        with self._lock:
            self.settings.auto_mode = enabled
            self._sync_detector()
        self._changed()

    def on_call_start(self, app: CallApp) -> None:
        with self._lock:
            if self._session is not None:
                return
            self._begin(app.value, EXE_FOR_APP[app], auto=True)

    def on_call_stop(self, app: CallApp) -> None:
        with self._lock:
            if self._session is not None and self._session_auto:
                self._stop_session(user=False)

    # --- interno -----------------------------------------------------------
    def _begin(self, label: str, exe: str | None, auto: bool) -> None:
        try:
            session = self._session_factory(label, exe, self._emit_line)
            session.start()
        except Exception as e:  # noqa: BLE001
            log.exception("No se pudo iniciar la grabación")
            self._notify("Error al grabar", str(e))
            return
        self._session, self._session_auto = session, auto
        self._sync_detector()
        for cb in list(self.on_session_start):
            cb(label)
        self._notify("Grabando llamada", f"{label} — transcripción en curso")
        self._changed()

    def _stop_session(self, user: bool) -> None:
        session, was_auto = self._session, self._session_auto
        self._session, self._session_auto = None, False
        try:
            path = session.stop()
            self.last_path = path
            self._notify("Transcripción guardada", str(path), path)
        except Exception as e:  # noqa: BLE001
            log.exception("Error cerrando la grabación")
            self._notify("Error al guardar", str(e))
        if was_auto and user:
            self._detector.dismiss()
        self._sync_detector()
        self._changed()

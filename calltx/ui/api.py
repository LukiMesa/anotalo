"""Puente entre la ventana (JavaScript) y la app. Todo devuelve datos JSON y nunca lanza excepciones a JS.

Ojo: pywebview expone a JS los métodos públicos; los atributos van con "_" para que no los recorra.
"""

from __future__ import annotations

import logging
import os
import shutil
import threading
from datetime import date, timedelta
from pathlib import Path
from typing import Callable

from .. import __version__
from ..paths import is_frozen
from ..app import CALL_LABEL
from ..library import Recording, Trash, list_recordings, parse_recording, to_readable

log = logging.getLogger(__name__)

MONTHS = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"]
EDITABLE = {
    "copy_on_stop", "show_indicator", "autostart", "auto_mode", "detect_teams", "detect_zoom", "detect_whatsapp", "detect_slack", "detect_discord",
    "detect_browsers",
    "label_me", "label_others", "model", "language", "output_dir", "hotkey", "release_after_min",
}
RELEASE_OPTIONS = {0, 2, 10, 30}  # minutos; 0 = nunca


def _dur(seconds: int | None) -> str:
    if seconds is None:
        return ""
    h, rem = divmod(seconds, 3600)
    return f"{h}:{rem // 60:02d}:{rem % 60:02d}" if h else f"{rem // 60:02d}:{rem % 60:02d}"


def _err(msg: str) -> dict:
    return {"ok": False, "error": msg}


class Api:
    def __init__(
        self,
        app,
        clipboard_set: Callable[[str], None],
        save_dialog: Callable[[str], Path | None],
        folder_dialog: Callable[[], Path | None] = lambda: None,
        today: Callable[[], date] = date.today,
        device_info: Callable[[], str] = lambda: "",
        hotkey_capture: Callable[[bool], None] = lambda on: None,
        chrome=None,
        updater=None,
    ):
        self._chrome = chrome
        self._updater = updater  # UpdateManager en la versión instalada; None en desarrollo
        self._run_bg = lambda fn: threading.Thread(target=fn, name="update-action", daemon=True).start()
        self._hotkey_capture = hotkey_capture
        self._capture_timeout_s = 20.0
        self._capture_timer: threading.Timer | None = None
        self._capturing = False
        self._capture_lock = threading.Lock()
        self._app = app
        self._copy = clipboard_set
        self._save_dialog = save_dialog
        self._folder_dialog = folder_dialog
        self._today = today
        self._device_info = device_info

    # --- utilidades -------------------------------------------------------
    def _folder(self) -> Path:
        return Path(self._app.settings.output_dir)

    def _path(self, rec_id: str) -> Path | None:
        if not rec_id or Path(rec_id).name != rec_id or not rec_id.endswith(".md"):
            return None
        p = self._folder() / rec_id
        return p if p.is_file() else None

    def _live_id(self) -> str | None:
        s = self._app.session
        p = getattr(s, "path", None) if s is not None else None
        return Path(p).name if p else None

    def _date_label(self, rec: Recording) -> str:
        if rec.started_at.year < 1900:
            return "Sin fecha"
        d, today = rec.started_at.date(), self._today()
        if d == today:
            return "Hoy"
        if d == today - timedelta(days=1):
            return "Ayer"
        label = f"{d.day} {MONTHS[d.month - 1]}"
        return label if d.year == today.year else f"{label} {d.year}"

    def _summary(self, rec: Recording, live_id: str | None) -> dict:
        has_time = rec.started_at.year >= 1900
        return {
            "id": rec.id,
            "kind": rec.kind,
            # Las llamadas grabadas a mano se guardan como "Llamada": se muestran como "Manual".
            "app": ("Manual" if rec.app == CALL_LABEL else rec.app) if rec.kind == "call" else "",
            "title": "Nota rápida" if rec.kind == "note" else "Llamada",
            "date_label": self._date_label(rec),
            "time": rec.started_at.strftime("%H:%M") if has_time else "",
            "duration": _dur(rec.duration_s),
            "snippet": rec.snippet,
            "live": rec.id == live_id,
        }

    # --- grabaciones ------------------------------------------------------
    def list_recordings(self, kind: str = "all", query: str = "") -> list[dict]:
        try:
            live = self._live_id()
            return [self._summary(r, live) for r in list_recordings(self._folder(), kind, query or "")]
        except Exception:  # noqa: BLE001
            log.exception("Error listando grabaciones")
            return []

    def get_recording(self, rec_id: str) -> dict:
        p = self._path(rec_id)
        if p is None:
            return _err("La grabación ya no existe")
        rec = parse_recording(p)
        out = self._summary(rec, self._live_id())
        me = self._app.settings.label_me
        start = rec.started_at.strftime("%H:%M") if rec.started_at.year >= 1900 else ""
        end = rec.ended_at.strftime("%H:%M") if rec.ended_at else ""
        out.update(
            ok=True,
            full_date=f"{rec.started_at.day} {MONTHS[rec.started_at.month - 1]} {rec.started_at.year}"
            if rec.started_at.year >= 1900 else "",
            start_end=f"{start} – {end}" if end else start,
            lines=[{"ts": l.ts, "speaker": l.speaker, "me": l.speaker == me, "text": l.text} for l in rec.lines],
        )
        return out

    def copy(self, rec_id: str) -> dict:
        p = self._path(rec_id)
        if p is None:
            return _err("La grabación ya no existe")
        try:
            self._copy(to_readable(parse_recording(p)))
            return {"ok": True}
        except Exception as e:  # noqa: BLE001
            return _err(f"No se pudo copiar: {e}")

    def save_as(self, rec_id: str) -> dict:
        p = self._path(rec_id)
        if p is None:
            return _err("La grabación ya no existe")
        dest = self._save_dialog(p.name)
        if not dest:
            return {"ok": False, "cancelled": True}
        try:
            shutil.copyfile(p, dest)
            return {"ok": True, "path": str(dest)}
        except OSError as e:
            return _err(f"No se pudo guardar: {e}")

    def delete(self, rec_id: str) -> dict:
        p = self._path(rec_id)
        if p is None:
            return _err("La grabación ya no existe")
        if rec_id == self._live_id():
            return _err("No se puede borrar mientras se está grabando")
        try:
            return {"ok": True, "token": Trash(self._folder()).delete(p)}
        except OSError as e:
            return _err(f"No se pudo borrar: {e}")

    def undo(self, token: str) -> dict:
        try:
            Trash(self._folder()).undo(token)
            return {"ok": True}
        except (OSError, ValueError) as e:
            return _err(f"No se pudo deshacer: {e}")

    def open_folder(self) -> dict:
        try:
            self._folder().mkdir(parents=True, exist_ok=True)
            os.startfile(self._folder())
            return {"ok": True}
        except OSError as e:
            return _err(str(e))

    # --- estado y ajustes -------------------------------------------------
    def get_state(self) -> dict:
        return {
            "state": self._app.state,
            "hotkey": self._app.settings.hotkey,
            "device": self._device_info(),
            "version": __version__,
            "frozen": is_frozen(),
            "kind": getattr(self._app, "session_kind", None),  # note | call | manual | None
            "update": self._updater.snapshot() if self._updater else None,
        }

    # --- actualizaciones -----------------------------------------------------
    def check_updates(self) -> dict:
        if self._updater is None:
            return _err("Solo en la versión instalada")
        self._run_bg(lambda: self._updater.check(user=True))
        return {"ok": True}

    def install_update(self) -> dict:
        if self._updater is None:
            return _err("Solo en la versión instalada")
        self._run_bg(self._updater.install)
        return {"ok": True}

    def toggle_call(self) -> dict:
        """Botón "Grabar llamada": empieza una llamada manual o para lo que se esté grabando."""
        try:
            self._app.toggle_call()
        except Exception as e:  # noqa: BLE001
            log.exception("Error en grabar llamada")
            return _err(str(e))
        return {"ok": True}

    def get_settings(self) -> dict:
        s = self._app.settings
        return {k: getattr(s, k) for k in sorted(EDITABLE)}

    def set_setting(self, key: str, value) -> dict:
        if key not in EDITABLE:
            return _err(f"Ajuste desconocido: {key}")
        if key == "release_after_min":
            try:
                value = int(value)
            except (TypeError, ValueError):
                value = None
            if value not in RELEASE_OPTIONS:
                return _err("Valor no válido")
        try:
            msg = self._app.apply_setting(key, value)
        except Exception as e:  # noqa: BLE001
            log.exception("Error aplicando ajuste %s", key)
            msg = str(e)
        return _err(msg) if msg else {"ok": True}

    def set_hotkey(self, spec: str) -> dict:
        return self.set_setting("hotkey", spec)

    def begin_hotkey_capture(self) -> dict:
        """Mientras la ventana escucha un atajo nuevo, el global se suelta para poder pulsarlo.
        Por seguridad se reanuda solo tras unos segundos aunque la ventana no avise."""
        with self._capture_lock:
            if not self._capturing:
                try:
                    self._hotkey_capture(True)
                except Exception:  # noqa: BLE001
                    log.exception("No se pudo pausar el atajo")
                self._capturing = True
            if self._capture_timer:
                self._capture_timer.cancel()
            self._capture_timer = threading.Timer(self._capture_timeout_s, self.end_hotkey_capture)
            self._capture_timer.daemon = True
            self._capture_timer.start()
        return {"ok": True}

    def end_hotkey_capture(self) -> dict:
        with self._capture_lock:
            if self._capture_timer:
                self._capture_timer.cancel()
                self._capture_timer = None
            if self._capturing:
                self._capturing = False
                try:
                    self._hotkey_capture(False)
                except Exception:  # noqa: BLE001
                    log.exception("No se pudo reanudar el atajo")
        return {"ok": True}

    def purge_trash(self, older_than_s: float = 5.0) -> dict:
        try:
            return {"ok": True, "purged": Trash(self._folder()).purge(older_than_s)}
        except OSError as e:
            return _err(str(e))

    def log_js(self, message: str) -> dict:
        log.warning("Ventana: %s", str(message)[:2000])
        return {"ok": True}

    # --- barra propia de la ventana -------------------------------------------
    def window_minimize(self) -> dict:
        self._chrome.minimize()
        return {"ok": True}

    def window_toggle_maximize(self) -> dict:
        return {"ok": True, "maximized": bool(self._chrome.toggle_maximize())}

    def window_close(self) -> dict:
        self._chrome.hide()
        return {"ok": True}

    def choose_folder(self) -> dict:
        p = self._folder_dialog()
        if not p:
            return {"ok": False, "cancelled": True}
        return self.set_setting("output_dir", str(p)) | {"path": str(p)}

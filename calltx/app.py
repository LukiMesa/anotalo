"""Coordinación: modo manual (atajo/icono) y modo automático (detector)."""

from __future__ import annotations

import logging
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Literal

from .config import Settings
from .detector import Detected

log = logging.getLogger(__name__)

State = Literal["idle", "armed", "recording", "preparing"]
NOTE_LABEL = "Nota"
CALL_LABEL = "Llamada"  # llamada grabada a mano ("Manual" ya es una etiqueta de nota antigua)


@dataclass(frozen=True)
class OthersSpec:
    """De dónde sale la voz de los demás: el audio de un programa o todo lo que suena en el equipo."""

    kind: Literal["process", "device"]
    exe: str | None = None


class App:
    def __init__(
        self,
        settings: Settings,
        detector_factory: Callable,
        session_factory: Callable,  # (label, OthersSpec|None, on_line, on_level) -> session; None = nota
        notifier: Callable[..., None],
        wait_ready: Callable[[float], bool] | None = None,
        copy_text: Callable[[str], None] = lambda text: None,
    ):
        self._copy_text = copy_text
        self.settings = settings
        self._detector = detector_factory(self.on_call_start, self.on_call_stop)
        self._session_factory = session_factory
        self._notify = notifier
        self._lock = threading.RLock()
        self._session = None
        self._session_auto = False
        self._others: OthersSpec | None = None
        self.on_state_change: list[Callable[[State], None]] = []
        self.on_line: list[Callable[[str, str, float, str], None]] = []
        self.on_session_start: list[Callable[[str], None]] = []
        self.on_session_end: list[Callable[[Path | None, str, bool], None]] = []
        self.on_level: list[Callable[[float], None]] = []
        self.on_session_stopping: list[Callable[[str], None]] = []
        self._label: str | None = None
        self.busy = False  # True mientras se cierra una grabación (últimas frases)
        self.preparing: tuple[float, str] | None = None  # (progreso 0-1, texto) en el primer uso
        self.progress: tuple[float, str] | None = None  # progreso visible (también recargas)
        self.request_model_reload: Callable[[], None] | None = None  # reintento si no hay modelo
        # ajuste → función que lo aplica en vivo (la rellena main); puede lanzar con un mensaje
        self.appliers: dict[str, Callable[[object], None]] = {}
        self.save_settings: Callable[[], None] = lambda: None
        # ajuste → acción tras guardarlo (p. ej. recargar el modelo)
        self.after_apply: dict[str, Callable[[object], None]] = {}
        self.last_path: Path | None = None
        self._wait_ready = wait_ready or (lambda timeout: True)
        self._stopping = False

    # --- estado ------------------------------------------------------------
    @property
    def state(self) -> State:
        if self._session is not None:
            return "recording"
        if self.preparing is not None:
            return "preparing"
        return "armed" if self.settings.auto_mode else "idle"

    def set_preparing(self, value: tuple[float, str] | None, blocking: bool = True) -> None:
        """blocking=False: solo informa del progreso (ya hay un modelo con el que grabar)."""
        self.preparing = value if blocking else None
        self.progress = value
        self._changed()

    @property
    def session(self):
        return self._session

    @property
    def session_kind(self) -> Literal["note", "call", "manual"] | None:
        if self._session is None:
            return None
        if self._others is None:
            return "note"
        return "call" if self._session_auto else "manual"

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
        self._stopping = True
        self._detector.stop()  # fuera del lock: el detector puede estar esperándolo
        with self._lock:
            if self._session is not None:
                self._stop_session(user=True)

    # --- acciones ----------------------------------------------------------
    def toggle(self) -> None:
        with self._lock:
            if self._session is not None:
                self._stop_session(user=True)
                return
        self.start_manual()

    def _model_ready(self) -> bool:
        """No bloquea: mientras se prepara el modelo (descargas del primer uso) no se graba."""
        prep = self.preparing  # copia: otro hilo puede cambiarlo
        if prep is None and self._wait_ready(0):
            return True
        if prep is None and self.request_model_reload is not None:
            # No hay modelo y no se está preparando (p. ej. falló la descarga sin conexión).
            self._notify("Modelo no disponible", "Reintentando la descarga; podrás grabar en cuanto termine")
            self.request_model_reload()
            return False
        pct = f" ({int(prep[0] * 100)} %)" if prep else ""
        self._notify("Aún preparando el modelo", f"Podrás grabar en cuanto termine{pct}")
        return False

    def start_manual(self) -> None:
        if self._stopping or not self._model_ready():
            return
        with self._lock:
            if self._session is not None or self._stopping:
                return
            self._begin(NOTE_LABEL, None, auto=False)

    def start_call_manual(self) -> None:
        """Grabar llamada a mano: micrófono + todo lo que suena en el equipo (con auriculares)."""
        if self._stopping or not self._model_ready():
            return
        with self._lock:
            if self._session is not None or self._stopping:
                return
            self._begin(CALL_LABEL, OthersSpec("device"), auto=False)

    def toggle_call(self) -> None:
        with self._lock:
            if self._session is not None:
                self._stop_session(user=True)
                return
        self.start_call_manual()

    def set_auto(self, enabled: bool) -> None:
        with self._lock:
            self.settings.auto_mode = enabled
            self._sync_detector()
        self._changed()

    def on_call_start(self, detected: Detected) -> None:
        if self._stopping or not self._model_ready():
            return
        with self._lock:
            if self._session is not None or self._stopping:
                return
            others = (OthersSpec("device") if detected.platform.audio == "device"
                      else OthersSpec("process", detected.exe))
            self._begin(detected.label, others, auto=True)

    def on_call_stop(self, detected: Detected) -> None:
        with self._lock:
            if self._session is not None and self._session_auto:
                self._stop_session(user=False)

    # --- interno -----------------------------------------------------------
    def _begin(self, label: str, others: OthersSpec | None, auto: bool) -> None:
        try:
            session = self._session_factory(label, others, self._emit_line, self._emit_level)
            session.start()
        except Exception as e:  # noqa: BLE001
            log.exception("No se pudo iniciar la grabación")
            self._notify("Error al grabar", str(e))
            return
        self._session, self._session_auto, self._label, self._others = session, auto, label, others
        self._sync_detector()
        for cb in list(self.on_session_start):
            cb(label)
        if label != NOTE_LABEL:
            self._notify("Grabando llamada", f"{label} — transcripción en curso")
        self._changed()

    def _stop_session(self, user: bool) -> None:
        session, was_auto, label = self._session, self._session_auto, self._label
        self._session, self._session_auto, self._label, self._others = None, False, None, None
        self.busy = True
        path, copied = None, False
        for cb in list(self.on_session_stopping):
            try:
                cb(label or "")
            except Exception:  # noqa: BLE001
                log.exception("Error en listener de parada")
        try:
            path = session.stop()
            self.last_path = path
            if label == NOTE_LABEL:
                copied = self._copy_note(session)
            else:
                self._notify("Transcripción guardada", str(path), path)
        except Exception as e:  # noqa: BLE001
            log.exception("Error cerrando la grabación")
            self._notify("Error al guardar", str(e))
        self.busy = False
        for cb in list(self.on_session_end):
            try:
                cb(path, label or "", copied)
            except Exception:  # noqa: BLE001
                log.exception("Error en listener de fin de grabación")
        if was_auto and user:
            self._detector.dismiss()
        self._sync_detector()
        self._changed()

    def _copy_note(self, session) -> bool:
        """Las notas rápidas se copian al portapapeles (solo el texto dicho). Las llamadas nunca."""
        if not self.settings.copy_on_stop:
            return False
        text = session.plain_text().strip()
        if not text:
            return False
        try:
            self._copy_text(text)
            return True
        except Exception:  # noqa: BLE001
            log.exception("No se pudo copiar la nota al portapapeles")
            return False

    def _emit_level(self, rms: float) -> None:
        for cb in list(self.on_level):
            try:
                cb(rms)
            except Exception:  # noqa: BLE001
                log.exception("Error en listener de nivel")

    # --- ajustes desde la ventana -----------------------------------------
    BLOCKED_WHILE_RECORDING = {"model", "language", "output_dir", "label_me", "label_others", "device"}

    def apply_setting(self, key: str, value) -> str | None:
        """Aplica un ajuste en vivo. Devuelve un mensaje de error o None si fue bien."""
        if not hasattr(self.settings, key):
            return f"Ajuste desconocido: {key}"
        with self._lock:
            if key in self.BLOCKED_WHILE_RECORDING and (self._session is not None or self.busy):
                return "No se puede cambiar mientras se graba"
            if key == "auto_mode":
                self.set_auto(bool(value))
            else:
                hook = self.appliers.get(key)
                if hook is not None:
                    try:
                        hook(value)
                    except Exception as e:  # noqa: BLE001 - el mensaje va a la ventana
                        return str(e) or "No se pudo aplicar el ajuste"
                setattr(self.settings, key, value)
        try:
            self.save_settings()
        except Exception:  # noqa: BLE001
            log.exception("No se pudieron guardar los ajustes")
        after = self.after_apply.get(key)
        if after is not None:
            try:
                after(value)
            except Exception:  # noqa: BLE001
                log.exception("Error tras aplicar %s", key)
        self._changed()
        return None

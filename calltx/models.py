"""Modelo de transcripción activo y su recarga segura desde Ajustes.

- Las recargas van de una en una.
- Mientras carga el nuevo, el anterior sigue disponible (no se bloquea la grabación).
- Si falla la carga, se queda el anterior.
- El modelo sustituido solo se para cuando no hay ninguna grabación usándolo.
"""

from __future__ import annotations

import logging
import threading
from typing import Any, Callable

log = logging.getLogger(__name__)


class ModelHolder:
    def __init__(self, factory: Callable[[], Any], busy: Callable[[], bool]):
        self._factory = factory
        self._busy = busy
        self._reload_lock = threading.Lock()
        self._lock = threading.Lock()
        self._retired: list[Any] = []
        self.current: Any = None
        self.ready = threading.Event()
        self.last_error: str | None = None

    def reload_sync(self, on_progress=None) -> bool:
        with self._reload_lock:
            new = self._factory()
            try:
                if on_progress is not None:
                    new.start(on_progress=on_progress)
                else:
                    new.start()
            except Exception as e:  # noqa: BLE001
                log.exception("No se pudo cargar el modelo")
                self.last_error = str(e)
                return False
            with self._lock:
                old, self.current = self.current, new
                if old is not None:
                    self._retired.append(old)
            self.last_error = None
            self.ready.set()
        self.on_idle()
        return True

    def reload(self) -> None:
        threading.Thread(target=self.reload_sync, name="model-load", daemon=True).start()

    def on_idle(self) -> None:
        """Llamar cuando termina una grabación: para los modelos sustituidos si nadie los usa."""
        if self._busy():
            return
        with self._lock:
            retired, self._retired = self._retired, []
        for t in retired:
            try:
                t.stop()
            except Exception:  # noqa: BLE001
                log.exception("Error parando el modelo anterior")

    def stop(self) -> None:
        with self._lock:
            items = self._retired + ([self.current] if self.current else [])
            self._retired = []
        for t in items:
            try:
                t.stop()
            except Exception:  # noqa: BLE001
                pass

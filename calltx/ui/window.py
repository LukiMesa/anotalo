"""Ventana principal (pywebview / WebView2). Cerrarla solo la oculta; la app sigue en la bandeja."""

from __future__ import annotations

import json
import logging
import threading
from pathlib import Path

import webview

log = logging.getLogger(__name__)

WEB_DIR = Path(__file__).parent / "web"


class MainWindow:
    def __init__(self, api, start_hidden: bool):
        self._loaded = threading.Event()
        self._quitting = False
        self.window = webview.create_window(
            "Transcriptor",
            url=str(WEB_DIR / "index.html"),
            js_api=api,
            width=1120,
            height=720,
            min_size=(940, 600),
            hidden=start_hidden,
            background_color="#161C2B",
            text_select=True,
        )
        self.window.events.loaded += lambda: self._loaded.set()
        self.window.events.closing += self._on_closing

    def _on_closing(self):
        if self._quitting:
            return True
        threading.Thread(target=self.window.hide, daemon=True).start()
        return False  # cancela el cierre: solo se oculta

    def show(self) -> None:
        try:
            self.window.show()
            self.window.restore()
        except Exception:  # noqa: BLE001
            log.exception("No se pudo mostrar la ventana")

    def push(self, event: dict) -> None:
        """Manda un evento a la página (no bloquea al que llama)."""
        if not self._loaded.is_set():
            return
        js = f"window.onEvent && window.onEvent({json.dumps(event, ensure_ascii=False)})"

        def run():
            try:
                self.window.run_js(js)
            except Exception:  # noqa: BLE001
                log.debug("No se pudo enviar evento a la ventana", exc_info=True)

        threading.Thread(target=run, daemon=True).start()

    def save_dialog(self, name: str) -> Path | None:
        res = self.window.create_file_dialog(
            webview.FileDialog.SAVE, save_filename=name, file_types=("Markdown (*.md)", "Todos los archivos (*.*)")
        )
        if not res:
            return None
        return Path(res if isinstance(res, str) else res[0])

    def folder_dialog(self) -> Path | None:
        res = self.window.create_file_dialog(webview.FileDialog.FOLDER)
        if not res:
            return None
        return Path(res if isinstance(res, str) else res[0])

    def destroy(self) -> None:
        self._quitting = True
        try:
            self.window.destroy()
        except Exception:  # noqa: BLE001
            pass

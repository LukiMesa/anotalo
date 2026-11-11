"""Ventana principal (pywebview / WebView2). Cerrarla solo la oculta; la app sigue en la bandeja."""

from __future__ import annotations

import json
import logging
import threading
from pathlib import Path

import webview

log = logging.getLogger(__name__)

from ..paths import web_dir

WEB_DIR = web_dir()


def enable_native_drag_regions() -> None:
    """WebView2: las zonas con CSS `app-region: drag` se comportan como barra de título
    nativa (arrastrar, encajar a los bordes, doble clic para maximizar)."""
    try:
        from webview.platforms import edgechromium
    except Exception:  # noqa: BLE001
        log.warning("No se pudo activar el arrastre nativo", exc_info=True)
        return
    if getattr(edgechromium.EdgeChrome, "_calltx_patched", False):
        return
    original = edgechromium.EdgeChrome.on_webview_ready

    def on_webview_ready(self, sender, args):
        original(self, sender, args)
        if args.IsSuccess:
            try:
                sender.CoreWebView2.Settings.IsNonClientRegionSupportEnabled = True
            except Exception:  # noqa: BLE001 - WebView2 antiguo: queda sin arrastre nativo
                log.warning("WebView2 sin soporte de zonas de arrastre", exc_info=True)

    edgechromium.EdgeChrome.on_webview_ready = on_webview_ready
    edgechromium.EdgeChrome._calltx_patched = True


def clear_web_cache(storage: Path) -> None:
    """Vacía la caché HTTP del navegador de la ventana. El servidor local de pywebview no manda
    cabeceras de no-caché, así que tras actualizar la app el navegador seguía mostrando la
    interfaz antigua. La página se sirve en local: vaciarla en cada arranque no cuesta nada."""
    import shutil

    default = Path(storage) / "EBWebView" / "Default"
    for name in ("Cache", "Code Cache"):
        shutil.rmtree(default / name, ignore_errors=True)


RESIZE_BORDER = 5


def install_frame(form, border: int = RESIZE_BORDER, on_state=lambda maximized: None) -> int:
    """Marco nativo (Windows anima minimizar/maximizar) y borde fino alrededor de la página
    para redimensionar. Devuelve el identificador de la ventana."""
    from System import Action
    from System.Windows.Forms import FormWindowState, Padding

    from . import nativeframe

    state: dict = {}

    def on_resize(sender, e):
        maximized = form.WindowState == FormWindowState.Maximized
        form.Padding = Padding(0 if maximized else border)  # maximizada no se redimensiona
        if state.get("max") != maximized:
            state["max"] = maximized
            on_state(maximized)

    def install():
        state["hwnd"] = int(form.Handle.ToInt64())
        nativeframe.install(state["hwnd"], border)
        form.Padding = Padding(border)
        form.Resize += on_resize

    form.Invoke(Action(install))
    return state["hwnd"]


class MainWindow:
    def __init__(self, api, start_hidden: bool):
        enable_native_drag_regions()
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
            background_color="#10141F",
            text_select=True,
            frameless=True,
            easy_drag=False,
        )
        self.hwnd: int | None = None
        self.chrome = None  # lo asigna main (Chrome)
        self.window.events.loaded += lambda: self._loaded.set()
        self.window.events.shown += self._on_shown
        self.window.events.closing += self._on_closing

    def _on_shown(self):
        from .winchrome import apply_chrome

        try:
            self.hwnd = install_frame(self.window.native,
                                      on_state=lambda m: self.push({"type": "window", "maximized": m}))
        except Exception:  # noqa: BLE001
            log.warning("No se pudo preparar el marco nativo", exc_info=True)
        if self.hwnd:
            apply_chrome(self.hwnd)

    def get_hwnd(self) -> int | None:
        """Identificador de la ventana (lo da la propia ventana al preparar su marco)."""
        return self.hwnd

    def hide(self) -> None:
        threading.Thread(target=self.window.hide, daemon=True).start()

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

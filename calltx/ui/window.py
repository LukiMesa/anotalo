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


RESIZE_BORDER = 5


def maximized_bounds(screen: tuple[int, int, int, int], work: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
    """Límites al maximizar (x, y, ancho, alto) relativos al monitor: su área de trabajo."""
    sx, sy, _, _ = screen
    wx, wy, ww, wh = work
    return wx - sx, wy - sy, ww, wh


def resize_bounds(edge: str, start: tuple[int, int, int, int], dx: int, dy: int,
                  min_w: int, min_h: int) -> tuple[int, int, int, int]:
    """Nuevo (x, y, ancho, alto) al arrastrar `edge` desde `start` un desplazamiento (dx, dy)."""
    x, y, w, h = start
    if "left" in edge:
        nw = max(min_w, w - dx)
        x, w = x + (w - nw), nw
    if "right" in edge:
        w = max(min_w, w + dx)
    if "top" in edge:
        nh = max(min_h, h - dy)
        y, h = y + (h - nh), nh
    if "bottom" in edge:
        h = max(min_h, h + dy)
    return x, y, w, h


def install_resize_border(form, border: int = RESIZE_BORDER, min_size: tuple[int, int] = (940, 600),
                          on_state=lambda maximized: None) -> None:
    """Deja un borde fino alrededor de la página que maneja la propia ventana para
    redimensionarla (la página procesa el ratón en otro proceso, así que no puede)."""
    from System import Action
    from System.Drawing import Rectangle
    from System.Windows.Forms import Control, Cursors, FormWindowState, MouseButtons, Padding, Screen

    cursors = {"left": Cursors.SizeWE, "right": Cursors.SizeWE, "top": Cursors.SizeNS, "bottom": Cursors.SizeNS,
               "topleft": Cursors.SizeNWSE, "bottomright": Cursors.SizeNWSE,
               "topright": Cursors.SizeNESW, "bottomleft": Cursors.SizeNESW}
    drag: dict = {}

    def edge_at(x: int, y: int) -> str | None:
        w, h = form.ClientSize.Width, form.ClientSize.Height
        c = border * 3
        v = "top" if y < c else "bottom" if y >= h - c else ""
        hz = "left" if x < c else "right" if x >= w - c else ""
        if v and hz:
            return v + hz
        if x < border:
            return "left"
        if x >= w - border:
            return "right"
        if y < border:
            return "top"
        if y >= h - border:
            return "bottom"
        return None

    def on_down(sender, e):
        edge = edge_at(e.X, e.Y)
        if e.Button == MouseButtons.Left and edge:
            p = Control.MousePosition
            b = form.Bounds
            drag.update(edge=edge, x0=p.X, y0=p.Y, start=(b.X, b.Y, b.Width, b.Height))
            form.Capture = True

    def on_move(sender, e):
        if drag:
            p = Control.MousePosition
            x, y, w, h = resize_bounds(drag["edge"], drag["start"], p.X - drag["x0"], p.Y - drag["y0"], *min_size)
            form.Bounds = Rectangle(x, y, w, h)
        else:
            form.Cursor = cursors.get(edge_at(e.X, e.Y), Cursors.Default)

    def on_up(sender, e):
        drag.clear()
        form.Capture = False

    def update_max_bounds(*_):
        scr = Screen.FromControl(form)
        b, w = scr.Bounds, scr.WorkingArea
        x, y, ww, hh = maximized_bounds((b.X, b.Y, b.Width, b.Height), (w.X, w.Y, w.Width, w.Height))
        form.MaximizedBounds = Rectangle(x, y, ww, hh)

    def on_resize(sender, e):
        maximized = form.WindowState == FormWindowState.Maximized
        form.Padding = Padding(0 if maximized else border)  # maximizada no se redimensiona
        if state.get("max") != maximized:
            state["max"] = maximized
            on_state(maximized)

    state: dict = {}

    def install():
        form.Padding = Padding(border)
        update_max_bounds()
        form.MouseDown += on_down
        form.MouseMove += on_move
        form.MouseUp += on_up
        form.Resize += on_resize
        form.LocationChanged += update_max_bounds

    form.Invoke(Action(install))


def set_border(form, border: int) -> None:
    from System import Action
    from System.Windows.Forms import Padding

    form.Invoke(Action(lambda: setattr(form, "Padding", Padding(border))))


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
        import os

        from .winchrome import apply_chrome, find_window

        self.hwnd = find_window("Transcriptor", os.getpid())
        if self.hwnd:
            apply_chrome(self.hwnd)
        try:
            install_resize_border(self.window.native,
                                  on_state=lambda m: self.push({"type": "window", "maximized": m}))
        except Exception:  # noqa: BLE001
            log.warning("No se pudo preparar el borde para redimensionar", exc_info=True)

    def get_hwnd(self) -> int | None:
        if not self.hwnd:
            import os

            from .winchrome import find_window

            self.hwnd = find_window("Transcriptor", os.getpid())
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

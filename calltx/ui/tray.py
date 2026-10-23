"""Icono en la bandeja del sistema."""

from __future__ import annotations

import threading
from typing import Callable

import pystray
from PIL import Image, ImageDraw

# Colores: acero oscuro (inactivo), órbita (esperando), error (grabando)
STATE_COLORS = {"idle": (107, 125, 146), "armed": (46, 197, 230), "recording": (255, 122, 122),
                "preparing": (90, 104, 131)}
STATE_TEXT = {"idle": "Inactivo", "armed": "Esperando llamada", "recording": "Grabando", "preparing": "Preparando…"}
NAVY = (22, 28, 43, 255)


_BASE: Image.Image | None = None


def _base() -> Image.Image:
    global _BASE
    if _BASE is None:
        from ..paths import assets_dir

        try:
            _BASE = Image.open(assets_dir() / "icon.png").convert("RGBA").resize((64, 64), Image.LANCZOS)
        except OSError:
            _BASE = Image.new("RGBA", (64, 64), NAVY)
    return _BASE


def _icon(color: tuple[int, int, int]) -> Image.Image:
    """Icono de la app con un punto de estado abajo a la derecha."""
    img = _base().copy()
    d = ImageDraw.Draw(img)
    d.ellipse((40, 40, 63, 63), fill=NAVY)
    d.ellipse((44, 44, 59, 59), fill=color + (255,))
    return img


def _bg(fn: Callable[[], None]) -> Callable[[], None]:
    """Las acciones del menú van en otro hilo para que el menú no se congele."""
    return lambda: threading.Thread(target=fn, daemon=True).start()


class Tray:
    def __init__(
        self,
        app,
        open_window: Callable[[], None],
        open_folder: Callable[[], None],
        on_quit: Callable[[], None],
    ):
        self._app = app
        menu = pystray.Menu(
            pystray.MenuItem("Abrir Transcriptor", _bg(open_window), default=True),
            pystray.MenuItem(
                lambda _: "Detener grabación" if app.state == "recording" else "Nueva nota rápida",
                _bg(app.toggle),
            ),
            pystray.MenuItem("Grabar llamadas automáticamente", _bg(lambda: app.apply_setting("auto_mode", not app.settings.auto_mode)),
                             checked=lambda _: app.settings.auto_mode),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Abrir carpeta de grabaciones", _bg(open_folder)),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Salir", _bg(on_quit)),
        )
        self._icon = pystray.Icon("calltx", _icon(STATE_COLORS[app.state]), "Transcriptor de llamadas", menu)
        app.on_state_change.append(self.update)

    def update(self, state: str) -> None:
        self._icon.icon = _icon(STATE_COLORS.get(state, STATE_COLORS["idle"]))
        self._icon.title = f"Transcriptor de llamadas — {STATE_TEXT.get(state, '')}"
        self._icon.update_menu()

    def set_tip(self, text: str | None) -> None:
        self._icon.title = f"Transcriptor de llamadas — {text}" if text else "Transcriptor de llamadas"

    def run_detached(self) -> None:
        self._icon.run_detached()

    def stop(self) -> None:
        self._icon.stop()

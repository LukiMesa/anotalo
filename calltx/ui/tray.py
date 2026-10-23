"""Icono en la bandeja del sistema."""

from __future__ import annotations

import threading
from typing import Callable

import pystray
from PIL import Image, ImageDraw

# Colores: acero oscuro (inactivo), órbita (esperando), error (grabando)
STATE_COLORS = {"idle": (107, 125, 146), "armed": (46, 197, 230), "recording": (255, 122, 122)}
STATE_TEXT = {"idle": "Inactivo", "armed": "Esperando llamada", "recording": "Grabando"}
NAVY = (22, 28, 43, 255)


def _icon(color: tuple[int, int, int]) -> Image.Image:
    img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.ellipse((4, 4, 60, 60), fill=color + (255,))
    d.rounded_rectangle((24, 13, 40, 37), radius=8, fill=NAVY)
    d.arc((17, 22, 47, 46), 0, 180, fill=NAVY, width=4)
    d.line((32, 46, 32, 52), fill=NAVY, width=4)
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
        self._icon.icon = _icon(STATE_COLORS[state])
        self._icon.title = f"Transcriptor de llamadas — {STATE_TEXT[state]}"
        self._icon.update_menu()

    def run_detached(self) -> None:
        self._icon.run_detached()

    def stop(self) -> None:
        self._icon.stop()

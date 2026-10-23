"""Icono en la bandeja del sistema."""

from __future__ import annotations

from typing import Callable

import pystray
from PIL import Image, ImageDraw

STATE_COLORS = {"idle": (130, 130, 130), "armed": (31, 111, 235), "recording": (220, 38, 38)}
STATE_TEXT = {"idle": "Inactivo", "armed": "Esperando llamada", "recording": "Grabando"}


def _icon(color: tuple[int, int, int]) -> Image.Image:
    img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.ellipse((4, 4, 60, 60), fill=color + (255,))
    d.rounded_rectangle((24, 14, 40, 38), radius=8, fill=(255, 255, 255, 255))
    d.arc((18, 24, 46, 46), 0, 180, fill=(255, 255, 255, 255), width=3)
    d.line((32, 46, 32, 52), fill=(255, 255, 255, 255), width=3)
    return img


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
            pystray.MenuItem(
                lambda _: "Detener grabación" if app.state == "recording" else "Iniciar grabación",
                lambda: app.toggle(),
                default=True,
            ),
            pystray.MenuItem("Modo automático", lambda: app.set_auto(not app.settings.auto_mode),
                             checked=lambda _: app.settings.auto_mode),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Ver transcripción en vivo", lambda: open_window()),
            pystray.MenuItem("Abrir carpeta de llamadas", lambda: open_folder()),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Salir", lambda: on_quit()),
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

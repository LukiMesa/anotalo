"""Punto de entrada: monta modelo, detector, icono, atajo y ventana."""

from __future__ import annotations

import ctypes
import logging
import os
import sys
import threading
import tkinter as tk
from logging.handlers import RotatingFileHandler
from pathlib import Path

from .app import App
from .config import default_config_path, load_settings, save_settings

log = logging.getLogger("calltx")


def _setup_logging(base: Path) -> None:
    base.mkdir(parents=True, exist_ok=True)
    h = RotatingFileHandler(base / "app.log", maxBytes=2_000_000, backupCount=2, encoding="utf-8")
    h.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s: %(message)s"))
    root = logging.getLogger()
    root.setLevel(logging.INFO)
    root.addHandler(h)
    if sys.stderr is not None:
        root.addHandler(logging.StreamHandler())


def _single_instance() -> bool:
    k32 = ctypes.windll.kernel32
    k32.CreateMutexW(None, False, "CallTranscriberMutex")
    return k32.GetLastError() != 183  # ERROR_ALREADY_EXISTS


def _notifier(title: str, msg: str, path: Path | None = None) -> None:
    def send():
        try:
            from winotify import Notification

            n = Notification(app_id="Transcriptor de llamadas", title=title, msg=msg)
            if path is not None:
                n.add_actions(label="Abrir transcripción", launch=Path(path).resolve().as_uri())
            n.show()
        except Exception:  # noqa: BLE001
            log.exception("No se pudo mostrar la notificación")

    threading.Thread(target=send, daemon=True).start()


def main() -> int:
    if not _single_instance():
        return 0
    cfg_path = default_config_path()
    _setup_logging(cfg_path.parent)
    settings = load_settings(cfg_path)
    if not cfg_path.exists():
        save_settings(settings, cfg_path)
    Path(settings.output_dir).mkdir(parents=True, exist_ok=True)

    from .asr import Transcriber
    from .audio.sources import DefaultLoopbackSource, MicSource, find_root_pid, open_call_audio
    from .detector import CallDetector, _is_firefox, _is_teams, mic_in_use
    from .session import CallSession
    from .ui.live_window import LiveWindow
    from .ui.tray import Tray

    transcriber = Transcriber(settings)
    ready = threading.Event()

    def load_model():
        log.info("Cargando modelo %s en %s…", settings.model, settings.device)
        try:
            transcriber.start()
        except Exception:  # noqa: BLE001
            log.exception("No se pudo cargar ningún modelo de transcripción")
            _notifier("Error", "No se pudo cargar el modelo de transcripción. Revisa app.log")
            return
        log.info("Modelo listo (%s%s)", transcriber.device, ", modo de respaldo" if transcriber.fallback_used else "")
        ready.set()

    threading.Thread(target=load_model, name="model-load", daemon=True).start()

    def manual_target() -> str | None:
        keys = mic_in_use()
        if any(_is_teams(k) for k in keys):
            return "ms-teams.exe"
        if any(_is_firefox(k) for k in keys):
            return "firefox.exe"
        return None

    def session_factory(label: str, exe: str | None, on_line, on_level=None):
        mic = MicSource(settings.mic_device)
        if exe is None:
            # Nota rápida: solo el micrófono.
            log.info("Nota rápida: solo micrófono")
            return CallSession(settings, transcriber, label, mic, None, on_line=on_line, on_level=on_level)

        def make_others():
            return open_call_audio(find_root_pid(exe))

        others = make_others()
        log.info("Sesión %s: audio de la llamada desde %s", label, others.name)
        return CallSession(
            settings, transcriber, label, mic, others,
            on_line=on_line, others_factory=make_others, on_level=on_level,
        )

    from .clipboard import set_text as copy_text

    app = App(
        settings, lambda on_start, on_stop: CallDetector(on_start, on_stop), session_factory, _notifier,
        wait_ready=lambda timeout: ready.wait(timeout), copy_text=copy_text,
    )

    root = tk.Tk()
    live = LiveWindow(root, settings.label_me, lambda: app.session.text() if app.session else "")
    app.on_line.append(lambda speaker, ts, _start, text: live.append(speaker, ts, text))
    app.on_session_start.append(lambda label: live.clear(f"Grabando — {label}"))
    app.on_state_change.append(
        lambda st: live.set_status({"idle": "Inactivo", "armed": "Esperando llamada…", "recording": "Grabando"}[st])
        if st != "recording" else None
    )

    def quit_all():
        def do():
            app.quit()
            save_settings(settings, cfg_path)
            tray.stop()
            transcriber.stop()
            root.after(0, root.destroy)

        threading.Thread(target=do, daemon=True).start()

    tray = Tray(app, live.show, lambda: os.startfile(settings.output_dir), quit_all)

    from .hotkey import GlobalHotkey

    hotkey = GlobalHotkey(settings.hotkey, lambda: threading.Thread(target=app.toggle, daemon=True).start())
    hotkey.start()

    app.start()
    tray.run_detached()
    log.info("Transcriptor iniciado (atajo %s, modo automático %s)", settings.hotkey, settings.auto_mode)
    try:
        root.mainloop()
    finally:
        hotkey.stop()
    return 0

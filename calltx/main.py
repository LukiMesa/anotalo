"""Punto de entrada: modelo, detector, bandeja, atajo, indicador y ventana principal."""

from __future__ import annotations

import ctypes
import logging
import os
import subprocess
import sys
import threading
from logging.handlers import RotatingFileHandler
from pathlib import Path

from .app import NOTE_LABEL, App
from .config import default_config_path, load_settings, save_settings

log = logging.getLogger("calltx")

ROOT = Path(__file__).resolve().parents[1]
from .paths import instance_names

MUTEX, SHOW_EVENT = instance_names()


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
    k32.CreateMutexW(None, False, MUTEX)
    return k32.GetLastError() != 183  # ERROR_ALREADY_EXISTS


def _signal_first_instance() -> None:
    k32 = ctypes.windll.kernel32
    h = k32.CreateEventW(None, False, False, SHOW_EVENT)
    if h:
        k32.SetEvent(h)
        k32.CloseHandle(h)


def _wait_show_requests(callback) -> None:
    k32 = ctypes.windll.kernel32
    k32.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    k32.CreateEventW.restype = ctypes.c_void_p
    h = k32.CreateEventW(None, False, False, SHOW_EVENT)

    def loop():
        while True:
            if k32.WaitForSingleObject(h, 0xFFFFFFFF) == 0:
                callback()

    threading.Thread(target=loop, name="show-requests", daemon=True).start()


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


def _gpu_name() -> str:
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=name", "--format=csv,noheader"],
            capture_output=True, text=True, timeout=5, creationflags=0x08000000,
        )
        return out.stdout.strip().splitlines()[0] if out.returncode == 0 and out.stdout.strip() else ""
    except (OSError, subprocess.SubprocessError):
        return ""


def selftest() -> int:
    """Comprobación rápida del paquete (la usa el script de compilación). Sin ventana ni audio."""
    from .paths import assets_dir, web_dir

    problems = []
    for f in [web_dir() / "index.html", web_dir() / "app.js", web_dir() / "fonts" / "Sora.ttf",
              web_dir() / "fonts" / "JetBrainsMono.ttf", assets_dir() / "icon.ico"]:
        if not f.is_file():
            problems.append(f"falta {f}")
    try:
        import numpy as np

        from .vad import SileroVad

        SileroVad().prob(np.zeros(512, np.float32))
    except Exception as e:  # noqa: BLE001
        problems.append(f"VAD: {e}")
    for mod in ("ctranslate2", "faster_whisper", "webview", "pystray", "comtypes", "pyaudiowpatch", "soxr"):
        try:
            __import__(mod)
        except Exception as e:  # noqa: BLE001
            problems.append(f"{mod}: {e}")
    lines = ["SELFTEST FALLO:"] + [f" - {p}" for p in problems] if problems else ["SELFTEST OK"]
    text = "\n".join(lines)
    if sys.stdout is not None:
        print(text)
    # el ejecutable no tiene consola: el resultado también va a un archivo (lo lee scripts/build.ps1)
    import tempfile

    Path(tempfile.gettempdir(), "transcriptor-selftest.txt").write_text(text, encoding="utf-8")
    return 1 if problems else 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    if not _single_instance():
        _signal_first_instance()
        return 0
    start_hidden = "--minimized" in sys.argv
    cfg_path = default_config_path()
    _setup_logging(cfg_path.parent)
    settings = load_settings(cfg_path)
    save_settings(settings, cfg_path)
    Path(settings.output_dir).mkdir(parents=True, exist_ok=True)

    import webview

    from . import autostart
    from .asr import Transcriber
    from .audio.sources import MicSource, find_root_pid, open_call_audio
    from .clipboard import set_text as copy_text
    from .detector import CallApp, CallDetector
    from .hotkey import GlobalHotkey, HotkeyError, validate_hotkey
    from .library import Trash
    from .session import CallSession
    from .ui.api import Api
    from .ui.indicator import Indicator, wire_indicator
    from .ui.tray import Tray
    from .ui.window import MainWindow
    from .ui.winchrome import Chrome

    def persist() -> None:
        save_settings(settings, cfg_path)

    # Arranque con Windows (primera vez con esta versión) y limpieza de la papelera.
    # Solo la versión instalada gestiona el arranque con Windows (desarrollo no lo toca).
    target = autostart.target_for_current_run()
    try:
        if target and autostart.ensure_autostart(settings, *target):
            persist()
    except Exception:  # noqa: BLE001
        log.exception("No se pudo configurar el arranque con Windows")
    try:
        Trash(Path(settings.output_dir)).purge(older_than_s=5)
    except OSError:
        log.exception("No se pudo vaciar la papelera")

    # --- modelo (se puede recargar desde Ajustes; ver calltx/models.py) -----
    from .models import ModelHolder

    from .cuda_setup import CudaSetup, CudaSetupError
    from .device import cpu_fallback, has_nvidia_driver, resolve
    from .paths import cuda_dir, is_frozen

    gpu = {"name": ""}
    holder_app: dict = {}
    plan = {"resolved": None}
    models = ModelHolder(lambda: Transcriber(settings, resolved=plan["resolved"]),
                         busy=lambda: holder_app["app"].state == "recording" or holder_app["app"].busy)

    load_lock = threading.Lock()
    last_pct = {"v": -1}

    def preparing(progress: float, text: str) -> None:
        pct = int(progress * 100)
        if pct == last_pct["v"]:
            return  # una actualización por punto porcentual
        last_pct["v"] = pct
        # Solo bloquea la grabación si todavía no hay ningún modelo cargado (primer uso).
        holder_app["app"].set_preparing((progress, text), blocking=models.current is None)
        tray_tip(f"Preparando… {pct} %")

    def load_model():
        if not load_lock.acquire(blocking=False):
            return  # ya hay una carga en marcha; usará los ajustes actuales
        try:
            _load_model()
        finally:
            load_lock.release()

    def _load_model():
        app_ = holder_app["app"]
        last_pct["v"] = -1
        app_.set_preparing((0.0, "Cargando modelo…"), blocking=models.current is None)
        nvidia = has_nvidia_driver()
        cuda = CudaSetup(cuda_dir())
        # En desarrollo las DLL de CUDA vienen del entorno de pip.
        cuda_ready = (not is_frozen()) or cuda.is_ready()
        r = resolve(settings, nvidia, cuda_ready)
        if r.needs_cuda:
            try:
                cuda.run(lambda pr, msg: preparing(pr * 0.6, msg))
            except CudaSetupError as e:
                log.warning("GPU no disponible en esta sesión: %s", e)
                _notifier("No se pudo preparar la GPU", "Esta vez uso el procesador; lo reintentaré al reiniciar.")
                r = cpu_fallback(settings)
        plan["resolved"] = r
        log.info("Cargando modelo %s en %s…", r.model, r.device)
        base = 0.6 if r.device == "cuda" and not cuda_ready else 0.0
        ok = models.reload_sync(on_progress=lambda pr, msg: preparing(base + pr * (1 - base), msg))
        if ok:
            t = models.current
            log.info("Modelo listo (%s%s)", t.device, ", modo de respaldo" if t.fallback_used else "")
        else:
            _notifier("Error", "No se pudo cargar el modelo de transcripción. Revisa app.log")
        app_.set_preparing(None)
        tray_tip(None)
        push_state()

    def tray_tip(text):
        t = holder_app.get("tray")
        if t is not None:
            t.set_tip(text)

    def device_info() -> str:
        t = models.current
        if t is None:
            return "Cargando modelo…"
        if t.device == "cuda":
            return f"GPU · {gpu['name'] or 'NVIDIA'} · {t.model_name}"
        return f"Procesador{' (respaldo)' if t.fallback_used else ''} · {t.model_name}"

    def session_factory(label: str, exe: str | None, on_line, on_level=None):
        mic = MicSource(settings.mic_device)
        if exe is None:
            log.info("Nota rápida: solo micrófono")
            return CallSession(settings, models.current, label, mic, None, on_line=on_line, on_level=on_level)

        def make_others():
            return open_call_audio(find_root_pid(exe))

        others = make_others()
        log.info("Sesión %s: audio de la llamada desde %s", label, others.name)
        return CallSession(settings, models.current, label, mic, others,
                           on_line=on_line, others_factory=make_others, on_level=on_level)

    def detector_factory(on_start, on_stop):
        return CallDetector(
            on_start, on_stop,
            enabled=lambda a: settings.detect_teams if a is CallApp.TEAMS else settings.detect_meet,
        )

    app = App(settings, detector_factory, session_factory, _notifier,
              wait_ready=lambda timeout: models.ready.wait(timeout), copy_text=copy_text)
    holder_app["app"] = app
    app.request_model_reload = lambda: threading.Thread(target=load_model, name="model-retry", daemon=True).start()
    app.on_session_end.append(lambda *_: models.on_idle())
    app.save_settings = persist

    # --- ventana -------------------------------------------------------------
    holder: dict = {}
    api = Api(
        app,
        clipboard_set=copy_text,
        save_dialog=lambda name: holder["win"].save_dialog(name),
        folder_dialog=lambda: holder["win"].folder_dialog(),
        device_info=device_info,
        hotkey_capture=lambda on: hotkey.pause() if on else hotkey.resume(),
        chrome=Chrome(lambda: holder["win"].get_hwnd(), hide=lambda: holder["win"].hide()),
    )
    win = MainWindow(api, start_hidden=start_hidden)
    holder["win"] = win
    win.chrome = api._chrome

    def push_state(*_):
        win.push({"type": "state", "state": app.state, "hotkey": settings.hotkey, "device": device_info(),
                  "progress": app.progress[0] if app.progress else None,
                  "progress_text": app.progress[1] if app.progress else ""})

    app.on_state_change.append(push_state)

    def on_line(speaker, ts, _start, text):
        s = app.session
        if s is not None and s.path is not None:
            win.push({"type": "line", "id": s.path.name, "ts": ts, "speaker": speaker,
                      "me": speaker == settings.label_me, "text": text})

    app.on_line.append(on_line)
    app.on_session_start.append(
        lambda label: win.push({"type": "recordings", "select": app.session.path.name if app.session and app.session.path else None})
    )

    # --- indicador -----------------------------------------------------------
    indicator = Indicator()
    wire_indicator(app, settings, indicator)
    app.on_session_end.append(lambda path, label, copied: win.push({"type": "recordings", "select": path.name if path else None}))

    # --- ajustes que se aplican en vivo ---------------------------------------
    hotkey = GlobalHotkey(settings.hotkey, lambda: threading.Thread(target=app.toggle, daemon=True).start())

    def apply_hotkey(spec):
        validate_hotkey(spec)
        try:
            hotkey.rebind(spec)
        except HotkeyError as e:
            raise ValueError(str(e)) from None

    def apply_autostart(on):
        if target is None:
            raise ValueError("Solo en la versión instalada")
        if on:
            autostart.enable(*target)
        else:
            autostart.disable()

    def apply_model(_value):
        threading.Thread(target=load_model, name="model-reload", daemon=True).start()

    def apply_output_dir(value):
        Path(value).mkdir(parents=True, exist_ok=True)

    app.appliers.update({
        "hotkey": apply_hotkey,
        "autostart": apply_autostart,
        "output_dir": apply_output_dir,
    })
    app.after_apply.update({
        "model": apply_model,
        "language": apply_model,
        "output_dir": lambda _v: win.push({"type": "recordings"}),
    })

    # --- bandeja y salida ----------------------------------------------------
    def quit_all():
        app.quit()
        persist()
        hotkey.stop()
        indicator.close()
        tray.stop()
        models.stop()
        win.destroy()

    tray = Tray(app, win.show, lambda: os.startfile(settings.output_dir), quit_all)
    holder_app["tray"] = tray
    _wait_show_requests(win.show)

    def bootstrap():
        gpu["name"] = _gpu_name()
        threading.Thread(target=load_model, name="model-load", daemon=True).start()
        hotkey.start()
        if not hotkey.registered.is_set():
            _notifier("Atajo no disponible", f"{settings.hotkey} lo usa otra app. Cámbialo en Ajustes.")
        app.start()
        tray.run_detached()
        push_state()
        log.info("Transcriptor iniciado (atajo %s, modo automático %s)", settings.hotkey, settings.auto_mode)

    storage = cfg_path.parent / "webview"
    from .paths import assets_dir

    webview.start(bootstrap, gui="edgechromium", private_mode=False, storage_path=str(storage),
                  icon=str(assets_dir() / "icon.ico"))
    return 0

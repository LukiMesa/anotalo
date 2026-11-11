"""Actualizar la versión instalada desde las Releases públicas de GitHub.

Se consulta la última Release (sin claves), se descarga su instalador comprobando que es
exactamente el publicado (dirección, tamaño y SHA-256) y se ejecuta en silencio: el instalador
cierra la app, la actualiza y la vuelve a abrir. Nunca durante una grabación ni sin un clic.
"""

from __future__ import annotations

import hashlib
import json
import logging
import re
import subprocess
import tempfile
import threading
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Literal

from . import __version__

log = logging.getLogger(__name__)

REPO = "LukiMesa/call-transcriber"  # tras renombrarlo a LukiMesa/anotalo, GitHub redirige esta ruta
API_LATEST = f"https://api.github.com/repos/{REPO}/releases/latest"
USER_AGENT = f"Transcriptor/{__version__}"
CHUNK = 1 << 16

State = Literal["idle", "checking", "available", "downloading", "waiting", "installing", "error", "latest"]


class UpdateError(Exception):
    pass


@dataclass(frozen=True)
class Release:
    version: str
    title: str
    url: str
    name: str
    size: int
    sha256: str | None


def parse_version(s: str) -> tuple[int, ...]:
    return tuple(int(x) for x in s.strip().lstrip("vV").split("."))


def is_newer(remote: str, local: str) -> bool:
    try:
        return parse_version(remote) > parse_version(local)
    except ValueError:
        return False


def _fetch_json(url: str) -> dict:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/vnd.github+json"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)


def _open(url: str):
    return urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": USER_AGENT}), timeout=60)


def fetch_latest(fetch_json: Callable[[str], dict] = _fetch_json) -> Release | None:
    """Última Release publicada con su instalador, o None si no tiene instalador."""
    data = fetch_json(API_LATEST)
    version = str(data["tag_name"]).lstrip("vV")
    assets = {a.get("name"): a for a in data.get("assets", [])}
    # Anotalo (la app reescrita) reemplaza a Transcriptor: se prefiere su instalador
    name = next((n for n in (f"Anotalo-Setup-{version}.exe", f"Transcriptor-Setup-{version}.exe") if n in assets), None)
    if name is None:
        return None
    asset = assets[name]
    digest = str(asset.get("digest") or "")
    sha = digest.split(":", 1)[1].lower() if digest.lower().startswith("sha256:") else None
    return Release(version, str(data.get("name") or data["tag_name"]), str(asset["browser_download_url"]),
                   name, int(asset["size"]), sha)


# Solo la página oficial (con su nombre actual o el antiguo) y solo el instalador de la app
_URL_OK = re.compile(r"^https://github\.com/LukiMesa/(?:call-transcriber|anotalo)/releases/download/"
                     r"v?[0-9.]+/((?:Transcriptor|Anotalo)-Setup-[0-9.]+\.exe)$")


def download(release: Release, dest_dir: Path, progress: Callable[[float], None], opener=_open) -> Path:
    """Descarga el instalador a dest_dir y comprueba que es el publicado. Si algo no cuadra,
    borra lo descargado y lanza UpdateError."""
    m = _URL_OK.match(release.url)
    if not m or m.group(1) != release.name:
        raise UpdateError("La descarga no viene de la página oficial de la app")
    dest_dir = Path(dest_dir)
    dest_dir.mkdir(parents=True, exist_ok=True)
    part = dest_dir / (release.name + ".part")
    final = dest_dir / release.name
    h = hashlib.sha256()
    done = 0
    try:
        with opener(release.url) as r, open(part, "wb") as f:
            while chunk := r.read(CHUNK):
                f.write(chunk)
                h.update(chunk)
                done += len(chunk)
                if release.size:
                    progress(min(1.0, done / release.size))
        if done != release.size:
            raise UpdateError("La descarga quedó incompleta")
        if release.sha256 and h.hexdigest() != release.sha256:
            raise UpdateError("El archivo descargado no coincide con el publicado")
        part.replace(final)
    except UpdateError:
        part.unlink(missing_ok=True)
        raise
    except Exception as e:  # noqa: BLE001 - red, disco…
        part.unlink(missing_ok=True)
        raise UpdateError(f"No se pudo descargar: {e}") from e
    progress(1.0)
    return final


def run_installer(path: Path, launcher=subprocess.Popen) -> None:
    """El instalador cierra esta app, la actualiza y la vuelve a abrir (mantiene las opciones)."""
    flags = getattr(subprocess, "DETACHED_PROCESS", 0) | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    launcher([str(path), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"], creationflags=flags, close_fds=True)


class UpdateManager:
    """Estado de la actualización para la ventana. Las llamadas son síncronas: main las lanza
    en hilos aparte."""

    def __init__(
        self,
        current: str,
        is_recording: Callable[[], bool],
        notify: Callable[..., None],
        on_change: Callable[[], None] = lambda: None,
        fetch: Callable[[], Release | None] = fetch_latest,
        downloader: Callable[..., Path] = download,
        installer: Callable[[Path], None] = run_installer,
        dest_dir: Path | None = None,
        notified_version: str = "",
        on_notified: Callable[[str], None] = lambda v: None,
    ):
        self.current = current
        self._is_recording = is_recording
        self._notify = notify
        self._on_change = on_change
        self._fetch = fetch
        self._download = downloader
        self._install = installer
        self._dest = Path(dest_dir) if dest_dir else Path(tempfile.gettempdir()) / "Transcriptor-update"
        self._notified = notified_version
        self._on_notified = on_notified
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self.state: State = "idle"
        self.release: Release | None = None
        self.progress: float | None = None
        self.error: str | None = None
        self._path: Path | None = None  # instalador ya descargado y comprobado
        self._installing_timeout_s = 120.0  # si la app sigue viva, el instalador falló
        self._timeout: threading.Timer | None = None

    def _set(self, state: State, error: str | None = None) -> None:
        self.state, self.error = state, error
        try:
            self._on_change()
        except Exception:  # noqa: BLE001
            log.exception("Error avisando del estado de la actualización")

    def snapshot(self) -> dict:
        r = self.release
        return {"state": self.state, "version": r.version if r else None, "title": r.title if r else None,
                "progress": self.progress, "error": self.error, "current": self.current}

    # --- comprobar ---------------------------------------------------------
    def check(self, user: bool = False) -> None:
        with self._lock:
            if self.state in ("downloading", "waiting", "installing"):
                return
            prev = self.state
            self._set("checking")
        try:
            rel = self._fetch()
        except Exception as e:  # noqa: BLE001 - sin red, límite de GitHub…
            log.info("No se pudo comprobar si hay actualizaciones: %s", e)
            if user:
                self._set("error", f"No se pudo comprobar: {e}")
            else:
                self._set(prev if prev in ("available", "latest") else "idle")
            return
        if rel is not None and is_newer(rel.version, self.current):
            self.release = rel
            self._set("available")
            if rel.version != self._notified:
                self._notified = rel.version
                self._notify("Actualización disponible", f"Versión {rel.version} lista para instalar desde la app")
                self._on_notified(rel.version)
        else:
            self._set("latest" if user else "idle")

    # --- instalar ----------------------------------------------------------
    def install(self) -> None:
        with self._lock:
            if self.release is None or self.state not in ("available", "error"):
                return
            if self._is_recording():
                self._set("waiting")
                return
            self._set("downloading")
        self._run()

    def on_recording_end(self) -> None:
        with self._lock:
            if self.state != "waiting":
                return
            self._set("downloading")
        self._run()

    def _run(self) -> None:
        rel = self.release
        try:
            if self._path is None:
                self.progress = 0.0
                self._path = self._download(rel, self._dest, self._progress)
            with self._lock:
                # La descarga tarda: puede haber empezado otra grabación mientras tanto.
                if self._is_recording():
                    self._set("waiting")
                    return
                self._set("installing")
            log.info("Instalando la versión %s", rel.version)
            self._install(self._path)
            self._arm_timeout()
        except Exception as e:  # noqa: BLE001
            log.warning("No se pudo actualizar: %s", e)
            self.progress = None
            self._path = None
            self._set("error", str(e))

    def _arm_timeout(self) -> None:
        """El instalador cierra esta app en segundos. Si sigue viva al rato, algo falló."""
        def expire():
            with self._lock:
                if self.state != "installing":
                    return
                self._path = None
                self._set("error", "La instalación no se completó. Vuelve a intentarlo.")

        self._timeout = threading.Timer(self._installing_timeout_s, expire)
        self._timeout.daemon = True
        self._timeout.start()

    def _progress(self, p: float) -> None:
        if self.progress is None or int(p * 100) != int(self.progress * 100):
            self.progress = p
            try:
                self._on_change()
            except Exception:  # noqa: BLE001
                pass

    # --- en segundo plano --------------------------------------------------
    def start_background(self, first_delay_s: float = 60, every_s: float = 6 * 3600) -> None:
        def loop():
            delay = first_delay_s
            while not self._stop.wait(delay):
                try:
                    self.check(user=False)
                except Exception:  # noqa: BLE001
                    log.exception("Error comprobando actualizaciones")
                delay = every_s

        self._thread = threading.Thread(target=loop, name="updater", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()


def announce_if_updated(settings, version: str, notify: Callable[..., None]) -> bool:
    """Tras actualizar, avisa una vez de la versión nueva. Devuelve True si cambió settings."""
    prev = settings.last_run_version
    if prev == version:
        return False
    settings.last_run_version = version
    if prev:
        notify("Transcriptor actualizado", f"Ya tienes la versión {version}")
    return True

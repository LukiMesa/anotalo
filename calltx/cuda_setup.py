"""Descarga de los componentes de GPU (CUDA) la primera vez, en PCs con NVIDIA.

Se bajan las mismas "wheels" de NVIDIA que usa el entorno de desarrollo desde PyPI,
se extraen solo las DLL y se marca la carpeta como lista al final. Si se corta, la
siguiente vez continúa donde lo dejó (descarga parcial *.part).
"""

from __future__ import annotations

import json
import logging
import shutil
import urllib.request
import zipfile
from pathlib import Path, PurePosixPath
from typing import Callable

log = logging.getLogger(__name__)

WHEELS: list[tuple[str, str]] = [
    ("nvidia-cublas-cu12", "12.9.2.10"),
    ("nvidia-cudnn-cu12", "9.27.0.42"),
    ("nvidia-cuda-nvrtc-cu12", "12.9.86"),
]
MARKER = "ready.json"
CHUNK = 1 << 20

Progress = Callable[[float, str], None]


class CudaSetupError(Exception):
    pass


def _fetch_json(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=30) as r:
        return json.load(r)


def _download(url: str, dest: Path, progress: Callable[[int, int], None]) -> None:
    start = dest.stat().st_size if dest.exists() else 0
    req = urllib.request.Request(url, headers={"Range": f"bytes={start}-"} if start else {})
    with urllib.request.urlopen(req, timeout=60) as r:
        if start and r.status != 206:  # el servidor no admite continuar: empezar de cero
            start = 0
            dest.unlink(missing_ok=True)
        total = start + int(r.headers.get("Content-Length") or 0)
        done = start
        with open(dest, "ab") as f:
            while chunk := r.read(CHUNK):
                f.write(chunk)
                done += len(chunk)
                progress(done, total)


class CudaSetup:
    def __init__(self, target: Path, fetch_json=_fetch_json, download=_download):
        self.target = Path(target)
        self._fetch_json = fetch_json
        self._download = download

    def is_ready(self) -> bool:
        try:
            data = json.loads((self.target / MARKER).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return False
        return [tuple(w) for w in data.get("wheels", [])] == WHEELS

    def _resolve(self) -> list[tuple[str, str, int]]:
        out = []
        for name, ver in WHEELS:
            info = self._fetch_json(f"https://pypi.org/pypi/{name}/{ver}/json")
            wheel = next((u for u in info["urls"] if u["filename"].endswith("win_amd64.whl")), None)
            if wheel is None:
                raise CudaSetupError(f"No hay versión para Windows de {name}")
            out.append((name, wheel["url"], int(wheel.get("size") or 0)))
        return out

    def run(self, progress: Progress) -> None:
        try:
            self._run(progress)
        except CudaSetupError:
            raise
        except Exception as e:  # noqa: BLE001
            log.exception("Fallo preparando la GPU")
            raise CudaSetupError(f"No se pudieron descargar los componentes de GPU: {e}") from e

    def _run(self, progress: Progress) -> None:
        self.target.mkdir(parents=True, exist_ok=True)
        (self.target / MARKER).unlink(missing_ok=True)
        dl = self.target / "_dl"
        dl.mkdir(exist_ok=True)
        wheels = self._resolve()
        total = sum(size for _, _, size in wheels) or 1
        base = 0
        for name, url, size in wheels:
            part = dl / f"{name}.whl.part"
            msg = f"Descargando componentes de GPU ({name.split('-')[1]})"

            def on_bytes(done: int, _tot: int, base=base, size=size, msg=msg) -> None:
                progress(min(1.0, (base + min(done, size or done)) / total) * 0.98, msg)

            self._download(url, part, on_bytes)
            self._extract(part, name)
            part.unlink(missing_ok=True)
            base += size
            progress(min(1.0, base / total) * 0.98, msg)
        shutil.rmtree(dl, ignore_errors=True)
        (self.target / MARKER).write_text(json.dumps({"wheels": WHEELS}), encoding="utf-8")
        progress(1.0, "Componentes de GPU listos")

    def _extract(self, wheel: Path, name: str) -> None:
        pkg = name.split("-")[1]  # cublas / cudnn / cuda
        out = self.target / pkg
        out.mkdir(exist_ok=True)
        try:
            with zipfile.ZipFile(wheel) as z:
                for info in z.infolist():
                    p = PurePosixPath(info.filename)
                    if p.suffix.lower() == ".dll" and "bin" in p.parts and p.parts[0] == "nvidia":
                        with z.open(info) as src, open(out / p.name, "wb") as dst:
                            shutil.copyfileobj(src, dst, CHUNK)
        except zipfile.BadZipFile as e:
            wheel.unlink(missing_ok=True)  # descarga corrupta: la próxima vez empieza de cero
            raise CudaSetupError(f"Descarga dañada de {name}") from e

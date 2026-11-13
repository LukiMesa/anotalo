"""El botón "Actualizar" de Transcriptor 0.6.4 hacia Anotalo, con el código de 0.6.4.

Lo usa scripts/test-setup-e2e.ps1 (CI). Recorre lo que hace 0.6.4 al pulsar el botón:
UpdateManager.check e install, que llaman a fetch_latest, download y run_installer de
calltx/updater.py. Solo se simula la red: la respuesta de /releases/latest es una Release 1.0.0
cuyo instalador es --setup, y la "descarga" lee ese archivo. Va a la carpeta de 0.6.4
(%TEMP%\\Transcriptor-update) y el instalador se lanza como lo lanza 0.6.4: suelto, con
/VERYSILENT /SUPPRESSMSGBOXES /NORESTART.

OJO: instala Anotalo de verdad en este usuario. Solo desde la prueba de punta a punta.

Uso: python -I scripts/fixtures/update_chain.py --setup <anotalo.exe>
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from functools import partial
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from calltx import updater  # noqa: E402
from calltx.updater import Release, UpdateManager  # noqa: E402

CURRENT = "0.6.4"
VERSION = "1.0.0"
NAME = f"Anotalo-Setup-{VERSION}.exe"
URL = f"https://github.com/LukiMesa/anotalo/releases/download/v{VERSION}/{NAME}"


def fail(msg: str) -> None:
    raise SystemExit(f"FALLO: {msg}")


def chain(setup: Path, dest: Path | None, installer, log=print) -> Path:
    """Actualiza 0.6.4 a la Release simulada con `setup` como instalador. `dest`: la carpeta de
    descarga (None: la de 0.6.4). `installer(path)` lanza el instalador. Devuelve su ruta."""
    data = Path(setup).read_bytes()
    sha = hashlib.sha256(data).hexdigest()
    expected = Release(VERSION, f"Anotalo {VERSION}", URL, NAME, len(data), sha)
    if not updater.is_newer(VERSION, CURRENT):
        fail(f"0.6.4 no ve {VERSION} como más nueva")

    asked: list[str] = []

    def fetch_json(url: str) -> dict:
        asked.append(url)
        return {"tag_name": f"v{VERSION}", "name": f"Anotalo {VERSION}", "draft": False, "prerelease": False,
                "assets": [{"name": NAME, "size": len(data), "digest": f"sha256:{sha}",
                            "browser_download_url": URL}]}

    rel = updater.fetch_latest(fetch_json)
    if rel != expected or asked != [updater.API_LATEST]:
        fail(f"0.6.4 leyó otra Release: {rel} (de {asked})")

    opened: list[str] = []

    def opener(url: str):
        opened.append(url)
        return open(setup, "rb")

    launched: list[Path] = []

    def install(path: Path) -> None:
        launched.append(path)
        log(f"0.6.4 lanza el instalador: {path} /VERYSILENT /SUPPRESSMSGBOXES /NORESTART")
        installer(path)

    def notify(title: str, body: str = "") -> None:
        log(f"Aviso de 0.6.4: {title}: {body}")

    shown = {"state": None, "quarter": -1}

    def on_change() -> None:
        # lo que la ventana de 0.6.4 enseñaría: el estado y la descarga de cuarto en cuarto
        quarter = int((m.progress or 0) * 4)
        if m.state != shown["state"] or (m.state == "downloading" and quarter != shown["quarter"]):
            shown.update(state=m.state, quarter=quarter)
            extra = f" {int((m.progress or 0) * 100)} %" if m.state == "downloading" else ""
            log(f"0.6.4: {m.state}{extra}")

    m = UpdateManager(
        CURRENT,
        is_recording=lambda: False,
        notify=notify,
        on_change=on_change,
        fetch=lambda: updater.fetch_latest(fetch_json),
        downloader=partial(updater.download, opener=opener),
        installer=install,
        dest_dir=dest,
    )
    m.check(user=True)
    if m.state != "available" or m.release != expected:
        fail(f"0.6.4 no ofrece la actualización: {m.snapshot()}")
    m.install()
    if m.state != "installing" or len(launched) != 1:
        fail(f"0.6.4 no se pudo instalar la actualización: {m.snapshot()}")
    path = launched[0]
    if opened != [URL] or path.name != NAME or path.read_bytes() != data:
        fail(f"0.6.4 no descargó el instalador publicado: {path} (de {opened})")
    log(f"Instalador comprobado por 0.6.4 (tamaño {len(data)}, SHA-256 {sha})")
    return path


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--setup", required=True, type=Path, help="el anotalo.exe que se instala")
    args = ap.parse_args(argv)
    setup = args.setup.resolve()
    if not setup.is_file():
        fail(f"no existe {setup}")
    chain(setup, None, updater.run_installer)
    return 0


if __name__ == "__main__":
    sys.exit(main())

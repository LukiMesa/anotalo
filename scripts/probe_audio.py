"""Comprobación manual: graba unos segundos del micro y del audio de la llamada y muestra el volumen.

Uso:
    python scripts/probe_audio.py --process firefox.exe
    python scripts/probe_audio.py --default
"""

import argparse
import logging
import sys
import threading
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from calltx.audio.sources import DefaultLoopbackSource, MicSource, ProcessLoopbackSource, find_root_pid  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--process", help="exe de la app de la llamada, p. ej. firefox.exe")
    g.add_argument("--default", action="store_true", help="loopback del dispositivo de salida por defecto")
    ap.add_argument("--seconds", type=float, default=8.0)
    args = ap.parse_args()
    logging.basicConfig(level=logging.INFO)

    if args.process:
        pid = find_root_pid(args.process)
        if not pid:
            print(f"{args.process} no está en ejecución")
            return 1
        others = ProcessLoopbackSource(pid)
    else:
        others = DefaultLoopbackSource()
    mic = MicSource()

    data = {"mic": [], "others": []}
    lock = threading.Lock()

    def sink(key):
        def cb(t, x):
            with lock:
                data[key].append(x)
        return cb

    mic.start(sink("mic"))
    others.start(sink("others"))
    t0 = time.monotonic()
    while time.monotonic() - t0 < args.seconds:
        time.sleep(1)
        with lock:
            for k, v in data.items():
                x = np.concatenate(v) if v else np.zeros(1, np.float32)
                print(f"{k:7s} {len(x) / 16000:5.1f}s  rms={np.sqrt(np.mean(x[-16000:] ** 2)):.4f}")
    mic.stop()
    others.stop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

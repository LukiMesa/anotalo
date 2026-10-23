"""Demo manual del indicador: grabando (niveles falsos) → transcribiendo → copiado.

Uso: python scripts/demo_indicator.py [--shot carpeta]  (guarda capturas de cada estado)
"""

import argparse
import ctypes
import math
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from calltx.ui.indicator import Indicator  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--shot", type=Path)
    args = ap.parse_args()
    fg_before = ctypes.windll.user32.GetForegroundWindow()
    ind = Indicator()

    def shot(name):
        if args.shot:
            from PIL import ImageGrab

            args.shot.mkdir(parents=True, exist_ok=True)
            img = ImageGrab.grab(all_screens=False)
            w, h = img.size
            img.crop((w // 2 - 260, h - 160, w // 2 + 260, h)).save(args.shot / f"{name}.png")

    ind.show_recording()
    t0 = time.monotonic()
    while time.monotonic() - t0 < 3:
        t = time.monotonic() - t0
        ind.push_level(abs(math.sin(t * 7)) * 0.06)
        time.sleep(0.03)
    shot("1-recording")
    ind.show_transcribing()
    time.sleep(1.0)
    shot("2-transcribing")
    ind.show_done(copied=True)
    time.sleep(0.6)
    shot("3-copied")
    time.sleep(1.5)
    fg_after = ctypes.windll.user32.GetForegroundWindow()
    print("foco intacto" if fg_before == fg_after else f"FOCO CAMBIÓ {fg_before} -> {fg_after}")
    ind.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

"""Lanzador sin consola: pythonw run.pyw"""

import multiprocessing
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from calltx.main import main  # noqa: E402

if __name__ == "__main__":  # el proceso de transcripción reimporta este archivo
    multiprocessing.freeze_support()
    raise SystemExit(main())

"""Punto de entrada del ejecutable empaquetado (Transcriptor.exe)."""

import multiprocessing
import sys

if __name__ == "__main__":
    # El proceso de transcripción es este mismo exe: freeze_support lo desvía antes de abrir la app.
    multiprocessing.freeze_support()
    from calltx.main import main

    sys.exit(main())

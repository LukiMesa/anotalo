"""Lanzador sin consola: pythonw run.pyw"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from calltx.main import main  # noqa: E402

raise SystemExit(main())

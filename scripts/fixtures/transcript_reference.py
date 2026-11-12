"""Transcripción de referencia escrita por la app de Python (calltx.transcript.TranscriptWriter).
El escritor de Anotalo debe producir exactamente los mismos bytes con las mismas entradas
(crates/core/tests/fixtures/transcript_python.md).

Uso (desde la raíz del repo): .venv/Scripts/python scripts/fixtures/transcript_reference.py
"""
import sys
import tempfile
from datetime import datetime
from pathlib import Path

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root))
from calltx.transcript import TranscriptWriter  # noqa: E402


class Clock:
    t = 0.0

    def __call__(self):
        return self.t


clock = Clock()
with tempfile.TemporaryDirectory() as d:
    w = TranscriptWriter(Path(d), "Teams", datetime(2026, 11, 11, 9, 5, 7), clock=clock)
    w.add("Otros", 1.2, "Hola, ¿me oyes?", end_s=2.4)
    w.add("Yo", 3.0, "Sí, perfecto.", end_s=3.8)
    w.add("Yo", 4.5, "Empezamos.", end_s=5.0)
    w.add("Otros", 30.0, "Vale.")
    clock.t = 10.0
    w.flush_due()
    data = w.close(datetime(2026, 11, 11, 9, 6, 0)).read_bytes()
out = root / "crates" / "core" / "tests" / "fixtures" / "transcript_python.md"
out.write_bytes(data)
print(len(data))

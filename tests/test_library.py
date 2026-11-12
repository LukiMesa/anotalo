import os
import time
from datetime import datetime

import pytest

from calltx.library import Trash, list_recordings, parse_recording, to_plain_text, to_readable

CALL = """# Llamada — Teams — 2026-10-23 15:04

- Inicio: 15:04:12
- Fin: 15:41:03
- Duración: 00:36:51

---

**[00:00:05] Yo:** Hola, ¿qué tal?

**[00:00:09] Otros:** Necesito cambiar el botón de contacto.

"""

NOTE = """# Llamada — Nota — 2026-10-23 11:20

- Inicio: 11:20:00
- Fin: 11:21:48
- Duración: 00:01:48

---

**[00:00:01] Yo:** Hola

**[00:00:04] Yo:** Segunda frase

"""

LEGACY = NOTE.replace("Nota", "Manual")

HALF = """# Llamada — Meet — 2026-10-22 17:45

- Inicio: 17:45:00

---

"""


@pytest.fixture
def folder(tmp_path):
    files = {
        "2026-10-23_1504_Teams.md": CALL,
        "2026-10-23_1120_Nota.md": NOTE,
        "2026-10-21_0900_Manual.md": LEGACY,
        "2026-10-22_1745_Meet.md": HALF,
        "basura.md": "\x00\x01 nada que ver",
    }
    for name, text in files.items():
        (tmp_path / name).write_text(text, encoding="utf-8")
    return tmp_path


def test_parse_call(folder):
    r = parse_recording(folder / "2026-10-23_1504_Teams.md")
    assert r.app == "Teams" and r.kind == "call"
    assert r.started_at == datetime(2026, 10, 23, 15, 4, 12)
    assert r.duration_s == 2211
    assert [l.speaker for l in r.lines] == ["Yo", "Otros"]
    assert r.lines[1].ts == "00:00:09" and r.lines[1].start_s == 9


def test_manual_is_note(folder):
    assert parse_recording(folder / "2026-10-21_0900_Manual.md").kind == "note"
    assert parse_recording(folder / "2026-10-23_1120_Nota.md").kind == "note"


def test_half_written_listed(folder):
    r = parse_recording(folder / "2026-10-22_1745_Meet.md")
    assert r.ended_at is None and r.duration_s is None and r.lines == []


def test_garbage_listed_not_raise(folder):
    r = parse_recording(folder / "basura.md")
    assert r.id == "basura.md"
    assert r.lines == []


def test_list_sorted_newest_first(folder):
    ids = [r.id for r in list_recordings(folder)]
    assert ids[:4] == [
        "2026-10-23_1504_Teams.md",
        "2026-10-23_1120_Nota.md",
        "2026-10-22_1745_Meet.md",
        "2026-10-21_0900_Manual.md",
    ]


def test_filter_kind(folder):
    assert {r.app for r in list_recordings(folder, kind="note")} == {"Nota", "Manual"}
    # el archivo sin formato se lista como llamada sin app (datos parciales)
    assert {r.app for r in list_recordings(folder, kind="call")} == {"Teams", "Meet", ""}


def test_search_accent_insensitive(folder):
    assert [r.id for r in list_recordings(folder, query="boton")] == ["2026-10-23_1504_Teams.md"]
    assert [r.id for r in list_recordings(folder, query="QUÉ TAL")] == ["2026-10-23_1504_Teams.md"]


def test_plain_text_note(folder):
    assert to_plain_text(parse_recording(folder / "2026-10-23_1120_Nota.md")) == "Hola\n\nSegunda frase"


def test_readable_format(folder):
    text = to_readable(parse_recording(folder / "2026-10-23_1504_Teams.md"))
    assert text.splitlines()[0] == "[00:00:05] Yo: Hola, ¿qué tal?"
    assert "**" not in text and "#" not in text


def test_trash_undo_roundtrip(folder):
    t = Trash(folder)
    p = folder / "2026-10-23_1120_Nota.md"
    token = t.delete(p)
    assert not p.exists()
    assert t.undo(token) == p and p.exists()


def test_two_deletes_undo_each(folder):
    t = Trash(folder)
    a, b = folder / "2026-10-23_1120_Nota.md", folder / "2026-10-23_1504_Teams.md"
    ta, tb = t.delete(a), t.delete(b)
    Trash(folder).undo(ta)  # otra instancia (ventana reabierta)
    assert a.exists() and not b.exists()
    Trash(folder).undo(tb)
    assert b.exists()


def test_purge_old(folder):
    t = Trash(folder)
    token = t.delete(folder / "2026-10-23_1120_Nota.md")
    old_token = f"{int((time.time() - 7200) * 1000)}__{token.split('__', 1)[1]}"
    os.replace(folder / ".papelera" / token, folder / ".papelera" / old_token)
    fresh = t.delete(folder / "2026-10-23_1504_Teams.md")
    assert t.purge(older_than_s=3600) == 1
    assert not (folder / ".papelera" / old_token).exists()
    assert (folder / ".papelera" / fresh).exists()


def test_papelera_not_listed(folder):
    Trash(folder).delete(folder / "2026-10-23_1120_Nota.md")
    assert "2026-10-23_1120_Nota.md" not in [r.id for r in list_recordings(folder)]


def test_bad_values_never_empty_the_list(tmp_path):
    (tmp_path / "2026-10-23_1300_Teams.md").write_text(CALL.replace("15:04:12", "25:00:00"), encoding="utf-8")
    (tmp_path / "2026-02-30_1000_Meet.md").write_text(HALF, encoding="utf-8")
    (tmp_path / "2026-10-23_1120_Nota.md").write_text(NOTE, encoding="utf-8")
    ids = {r.id for r in list_recordings(tmp_path)}
    assert ids == {"2026-10-23_1300_Teams.md", "2026-02-30_1000_Meet.md", "2026-10-23_1120_Nota.md"}


def test_reads_anotalo_title_and_marks():
    """Un archivo de Anotalo con título y marcas se sigue leyendo en la app de Python."""
    from pathlib import Path

    fixture = Path(__file__).resolve().parents[1] / "crates" / "core" / "tests" / "fixtures" / "transcript_rust_title_marks.md"
    rec = parse_recording(fixture)
    assert rec.kind == "call" and rec.app == "Teams"
    assert [(l.speaker, l.text) for l in rec.lines] == [("Otros", "Hola, ¿empezamos?"), ("Yo", "Sí, adelante.")]
    assert rec.duration_s == 53


def test_reads_anotalo_named_speakers():
    """Un archivo de Anotalo con personas separadas y nombradas se sigue leyendo en la app de Python."""
    from pathlib import Path

    fixture = Path(__file__).resolve().parents[1] / "crates" / "core" / "tests" / "fixtures" / "transcript_rust_speakers.md"
    rec = parse_recording(fixture)
    assert [l.speaker for l in rec.lines] == ["María", "Yo", "Persona 1", "María"]
    assert rec.duration_s == 60


def test_python_reads_rust_client_header():
    """Un archivo de Anotalo con título, cliente, marcas y personas se sigue leyendo en la app de Python."""
    from pathlib import Path

    fixture = (
        Path(__file__).resolve().parents[1] / "crates" / "core" / "tests" / "fixtures" / "library" / "2026-11-12_1000_Teams.md"
    )
    rec = parse_recording(fixture)
    assert rec.kind == "call" and rec.app == "Teams"
    assert rec.started_at == datetime(2026, 11, 12, 10, 0, 0)
    assert rec.duration_s == 60
    assert [l.speaker for l in rec.lines] == ["María", "Yo", "Persona 1", "María"]
    assert rec.lines[0].text == "Hola, soy María."
    assert [r.id for r in list_recordings(fixture.parent)][:4] == [
        "2026-11-12_1000_Teams.md",
        "2026-11-11_0905_Meet.md",
        "2026-11-10_1120_Nota.md",
        "2026-10-23_1504_Teams.md",
    ]

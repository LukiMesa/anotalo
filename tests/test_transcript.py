from datetime import datetime

from calltx.transcript import TranscriptWriter

START = datetime(2026, 10, 23, 15, 4, 12)
END = datetime(2026, 10, 23, 15, 41, 3)


class Clock:
    def __init__(self):
        self.t = 0.0

    def __call__(self):
        return self.t


def make(tmp_path, app="Teams"):
    clock = Clock()
    return TranscriptWriter(tmp_path, app, START, clock=clock), clock


def test_filename(tmp_path):
    w, _ = make(tmp_path)
    assert w.path.name == "2026-10-23_1504_Teams.md"


def test_line_format(tmp_path):
    w, _ = make(tmp_path)
    w.add("Yo", 5.2, "Hola")
    text = w.close(END).read_text(encoding="utf-8")
    assert "**[00:00:05] Yo:** Hola" in text


def test_reorders_by_start(tmp_path):
    w, clock = make(tmp_path)
    w.add("Otros", 10.0, "segundo", end_s=11.0)
    w.add("Yo", 8.0, "primero", end_s=9.0)
    clock.t = 10.0
    w.flush_due()
    text = w.path.read_text(encoding="utf-8")
    assert text.index("primero") < text.index("segundo")


def test_merge_same_speaker_short_gap(tmp_path):
    w, _ = make(tmp_path)
    w.add("Otros", 1.0, "Necesito", end_s=2.0)
    w.add("Otros", 2.5, "un botón azul.", end_s=4.0)
    text = w.close(END).read_text(encoding="utf-8")
    assert "**[00:00:01] Otros:** Necesito un botón azul." in text
    assert text.count("Otros:**") == 1


def test_no_merge_long_gap(tmp_path):
    w, _ = make(tmp_path)
    w.add("Otros", 1.0, "Uno", end_s=2.0)
    w.add("Otros", 6.0, "Dos", end_s=7.0)
    text = w.close(END).read_text(encoding="utf-8")
    assert text.count("Otros:**") == 2


def test_empty_text_ignored(tmp_path):
    w, _ = make(tmp_path)
    assert w.add("Yo", 1.0, "   ") is None
    text = w.close(END).read_text(encoding="utf-8")
    assert "Yo:**" not in text


def test_lines_on_disk_before_close(tmp_path):
    w, clock = make(tmp_path)
    w.add("Yo", 1.0, "Hola cliente", end_s=2.0)
    clock.t = 3.5
    w.flush_due()
    with open(w.path, encoding="utf-8") as f:
        assert "**[00:00:01] Yo:** Hola cliente" in f.read()


def test_not_flushed_before_hold(tmp_path):
    w, clock = make(tmp_path)
    w.add("Yo", 1.0, "pronto", end_s=2.0)
    clock.t = 1.0
    w.flush_due()
    assert "pronto" not in w.path.read_text(encoding="utf-8")


def test_close_writes_duration(tmp_path):
    w, _ = make(tmp_path)
    text = w.close(END).read_text(encoding="utf-8")
    assert "# Llamada — Teams — 2026-10-23 15:04" in text
    assert "- Inicio: 15:04:12" in text
    assert "- Fin: 15:41:03" in text
    assert "- Duración: 00:36:51" in text


def test_name_collision(tmp_path):
    a, _ = make(tmp_path)
    b, _ = make(tmp_path)
    assert a.path != b.path
    assert b.path.name == "2026-10-23_1504_Teams_2.md"


def test_long_merged_run_reaches_disk(tmp_path):
    # El cliente habla 60 s con pausas cortas: lo dicho debe llegar a disco sin esperar a que calle.
    w, clock = make(tmp_path)
    for i in range(30):
        clock.t = i * 2.0
        w.add("Otros", i * 2.0, f"frase{i}", end_s=i * 2.0 + 1.5)
        w.flush_due()
    assert "frase0" in w.path.read_text(encoding="utf-8")

from datetime import date
from pathlib import Path

import pytest

from calltx.config import Settings
from calltx.library import Trash
from calltx.ui.api import Api

CALL = """# Llamada — Teams — 2026-10-23 15:04

- Inicio: 15:04:12
- Fin: 15:41:03
- Duración: 00:36:51

---

**[00:00:05] Yo:** Hola, ¿qué tal?

**[00:00:09] Otros:** Necesito cambiar el botón.

"""

NOTE = """# Llamada — Nota — 2026-10-22 11:20

- Inicio: 11:20:00
- Fin: 11:21:48
- Duración: 00:01:48

---

**[00:00:01] Yo:** Apuntar factura

"""


class FakeApp:
    def __init__(self):
        self.settings = Settings()
        self.state = "armed"
        self.session = None
        self.applied = []

    def apply_setting(self, key, value):
        if key == "hotkey" and value == "r":
            return "El atajo necesita Ctrl, Alt o Win"
        if key not in vars(self.settings):
            return f"Ajuste desconocido: {key}"
        self.applied.append((key, value))
        setattr(self.settings, key, value)
        return None


@pytest.fixture
def env(tmp_path):
    (tmp_path / "2026-10-23_1504_Teams.md").write_text(CALL, encoding="utf-8")
    (tmp_path / "2026-10-22_1120_Nota.md").write_text(NOTE, encoding="utf-8")
    copied, saved_to = [], {}
    app = FakeApp()
    app.settings.output_dir = str(tmp_path)

    def save_dialog(name):
        return saved_to.get("path")

    api = Api(app, clipboard_set=copied.append, save_dialog=save_dialog, today=lambda: date(2026, 10, 23))
    return api, app, tmp_path, copied, saved_to


def test_list_dicts_labels(env):
    api, *_ = env
    items = api.list_recordings()
    assert [i["date_label"] for i in items] == ["Hoy", "Ayer"]
    first = items[0]
    assert first["id"] == "2026-10-23_1504_Teams.md"
    assert first["kind"] == "call" and first["app"] == "Teams"
    assert first["time"] == "15:04" and first["duration"] == "36:51"
    assert first["title"] == "Llamada" and items[1]["title"] == "Nota rápida"
    assert "botón" in first["snippet"]


def test_list_filter_and_query(env):
    api, *_ = env
    assert [i["id"] for i in api.list_recordings("note")] == ["2026-10-22_1120_Nota.md"]
    assert [i["id"] for i in api.list_recordings("all", "boton")] == ["2026-10-23_1504_Teams.md"]


def test_get_recording_lines_me_flag(env):
    api, *_ = env
    r = api.get_recording("2026-10-23_1504_Teams.md")
    assert r["start_end"] == "15:04 – 15:41"
    assert [(l["speaker"], l["me"]) for l in r["lines"]] == [("Yo", True), ("Otros", False)]
    assert r["lines"][0]["ts"] == "00:00:05"


def test_copy_uses_readable(env):
    api, _, _, copied, _ = env
    assert api.copy("2026-10-23_1504_Teams.md") == {"ok": True}
    assert copied == ["[00:00:05] Yo: Hola, ¿qué tal?\n[00:00:09] Otros: Necesito cambiar el botón."]


def test_delete_undo_via_api(env):
    api, _, folder, *_ = env
    res = api.delete("2026-10-22_1120_Nota.md")
    assert res["ok"] and not (folder / "2026-10-22_1120_Nota.md").exists()
    assert api.undo(res["token"]) == {"ok": True}
    assert (folder / "2026-10-22_1120_Nota.md").exists()


def test_delete_live_recording_refused(env):
    api, app, folder, *_ = env

    class S:
        path = folder / "2026-10-23_1504_Teams.md"

    app.session = S()
    res = api.delete("2026-10-23_1504_Teams.md")
    assert res["ok"] is False and "grabando" in res["error"]


def test_set_hotkey_invalid_returns_error(env):
    api, *_ = env
    assert api.set_hotkey("r") == {"ok": False, "error": "El atajo necesita Ctrl, Alt o Win"}
    assert api.set_hotkey("<ctrl>+<alt>+n") == {"ok": True}


def test_set_setting_unknown_key_error(env):
    api, *_ = env
    res = api.set_setting("nope", 1)
    assert res["ok"] is False and "nope" in res["error"]


def test_save_as_cancel(env):
    api, _, _, _, saved_to = env
    assert api.save_as("2026-10-23_1504_Teams.md") == {"ok": False, "cancelled": True}


def test_save_as_writes_markdown(env, tmp_path_factory):
    api, _, folder, _, saved_to = env
    dest = tmp_path_factory.mktemp("out") / "copia.md"
    saved_to["path"] = dest
    assert api.save_as("2026-10-23_1504_Teams.md") == {"ok": True, "path": str(dest)}
    assert dest.read_text(encoding="utf-8") == CALL


def test_unknown_id_is_error_not_exception(env):
    api, *_ = env
    assert api.get_recording("../../etc.md")["ok"] is False
    assert api.copy("no-existe.md")["ok"] is False


def test_hotkey_capture_hook(env):
    api, app, *_ = env
    calls = []
    api._hotkey_capture = calls.append
    assert api.begin_hotkey_capture() == {"ok": True}
    assert api.end_hotkey_capture() == {"ok": True}
    assert calls == [True, False]


def test_log_js_never_raises(env):
    api, *_ = env
    assert api.log_js("TypeError: x is undefined") == {"ok": True}


def test_hotkey_capture_auto_resumes(env):
    import time

    api, *_ = env
    calls = []
    api._hotkey_capture = calls.append
    api._capture_timeout_s = 0.2
    api.begin_hotkey_capture()
    time.sleep(0.5)
    assert calls == [True, False]
    api.end_hotkey_capture()  # no duplica la reanudación
    assert calls == [True, False]


def test_purge_trash_removes_old(env):
    import time

    api, _, folder, *_ = env
    tok = api.delete("2026-10-22_1120_Nota.md")["token"]
    time.sleep(0.05)
    assert api.purge_trash(0.01) == {"ok": True, "purged": 1}
    assert api.undo(tok)["ok"] is False


def test_undo_token_validated(env):
    api, *_ = env
    assert api.undo(r"1__..\..\x.md")["ok"] is False


def test_api_window_controls_delegate(env):
    api, *_ = env
    calls = []

    class C:
        maximized = False

        def minimize(self): calls.append("min")
        def toggle_maximize(self): calls.append("max"); return True
        def hide(self): calls.append("hide")

    api._chrome = C()
    api.window_minimize()
    assert api.window_toggle_maximize() == {"ok": True, "maximized": True}
    api.window_close()
    assert calls == ["min", "max", "hide"]
    assert not hasattr(api, "window_start_drag") and not hasattr(api, "window_start_resize")


def test_release_after_saved_as_int(tmp_path):
    app = FakeApp()
    api = Api(app, clipboard_set=lambda t: None, save_dialog=lambda n: None)
    assert api.get_settings()["release_after_min"] == 10
    assert api.set_setting("release_after_min", "2") == {"ok": True}
    assert app.settings.release_after_min == 2
    assert api.set_setting("release_after_min", 0) == {"ok": True}
    assert app.settings.release_after_min == 0


@pytest.mark.parametrize("bad", ["5", "x", None, -1])
def test_release_after_rejects_other_values(bad):
    app = FakeApp()
    api = Api(app, clipboard_set=lambda t: None, save_dialog=lambda n: None)
    assert api.set_setting("release_after_min", bad)["ok"] is False
    assert app.settings.release_after_min == 10


def test_api_toggle_call_delegates(env):
    api, app, *_ = env
    calls = []
    app.toggle_call = lambda: calls.append("call")
    assert api.toggle_call() == {"ok": True}
    assert calls == ["call"]


def test_get_state_reports_kind(env):
    api, app, *_ = env
    app.session_kind = "manual"
    assert api.get_state()["kind"] == "manual"
    app.session_kind = None
    assert api.get_state()["kind"] is None


def test_platform_settings_editable(env):
    api, app, *_ = env
    for key in ("detect_zoom", "detect_whatsapp", "detect_slack", "detect_discord", "detect_browsers"):
        assert api.set_setting(key, True) == {"ok": True}
    assert "detect_meet" not in api.get_settings()


def test_manual_call_shows_as_manual(env):
    api, app, folder, *_ = env
    (folder / "2026-10-23_1700_Llamada.md").write_text(CALL.replace("Teams", "Llamada"), encoding="utf-8")
    rec = next(r for r in api.list_recordings() if r["id"] == "2026-10-23_1700_Llamada.md")
    assert rec["kind"] == "call" and rec["app"] == "Manual"

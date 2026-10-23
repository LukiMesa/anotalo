from calltx.config import Settings
from calltx.ui.indicator import wire_indicator


class FakeInd:
    def __init__(self):
        self.calls = []

    def __getattr__(self, name):
        return lambda *a: self.calls.append((name, *a))


class FakeApp:
    def __init__(self):
        self.on_session_start, self.on_level, self.on_session_stopping, self.on_session_end = [], [], [], []
        self.after_apply = {}


def test_end_always_clears_pill_even_if_disabled_midway():
    app, ind, s = FakeApp(), FakeInd(), Settings()
    wire_indicator(app, s, ind)
    app.on_session_start[0]("Nota")
    s.show_indicator = False
    app.after_apply["show_indicator"](False)
    for cb in app.on_session_stopping:
        cb("Nota")
    for cb in app.on_session_end:
        cb(None, "Nota", True)
    names = [c[0] for c in ind.calls]
    assert names[0] == "show_recording"
    assert "hide" in names and "show_transcribing" not in names


def test_failed_save_hides_instead_of_saved():
    app, ind, s = FakeApp(), FakeInd(), Settings()
    wire_indicator(app, s, ind)
    app.on_session_start[0]("Teams")
    for cb in app.on_session_end:
        cb(None, "Teams", False)
    assert ind.calls[-1] == ("hide",)


def test_render_error_does_not_kill_indicator(monkeypatch):
    import time

    from calltx.ui import indicator as mod

    calls = {"n": 0}
    real = mod.render_pill

    def flaky(*a, **k):
        calls["n"] += 1
        if calls["n"] == 1:
            raise RuntimeError("boom")
        return real(*a, **k)

    monkeypatch.setattr(mod, "render_pill", flaky)
    ind = mod.Indicator()
    ind.show_recording()
    time.sleep(0.4)
    alive = ind._thread.is_alive()
    ind.close()
    assert alive and calls["n"] > 1

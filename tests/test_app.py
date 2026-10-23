from pathlib import Path

from calltx.app import App
from calltx.config import Settings
from calltx.detector import CallApp


class FakeDetector:
    def __init__(self, on_start, on_stop):
        self.on_start, self.on_stop = on_start, on_stop
        self.suspended = False
        self.dismissed = 0
        self.running = False

    def run(self):
        self.running = True

    def stop(self):
        self.running = False

    def suspend(self):
        self.suspended = True

    def resume(self):
        self.suspended = False

    def dismiss(self):
        self.dismissed += 1


class FakeSession:
    def __init__(self, label, exe, text="texto de Yo"):
        self.label, self.exe = label, exe
        self.started = self.stopped = False
        self._text = text

    def plain_text(self):
        return self._text

    def start(self):
        self.started = True

    def stop(self):
        self.stopped = True
        return Path(f"{self.label}.md")

    def text(self):
        return ""


class Harness:
    def __init__(self, auto=True, text="texto de Yo", copy_on_stop=True):
        self.sessions = []
        self.notes = []
        self.copied = []
        self.ended = []

        def sf(label, exe, on_line, on_level):
            s = FakeSession(label, exe, text)
            self.sessions.append(s)
            return s

        def df(on_start, on_stop):
            self.det = FakeDetector(on_start, on_stop)
            return self.det

        self.app = App(Settings(auto_mode=auto, copy_on_stop=copy_on_stop), df, sf,
                       lambda *a: self.notes.append(a), copy_text=self.copied.append)
        self.app.on_session_end.append(lambda *a: self.ended.append(a))
        self.app.start()


def test_toggle_starts_and_stops():
    h = Harness()
    h.app.toggle()
    assert h.app.state == "recording"
    s = h.sessions[0]
    assert s.started and s.label == "Nota"
    h.app.toggle()
    assert s.stopped
    assert h.app.state == "armed"


def test_manual_suspends_detector():
    h = Harness()
    h.app.toggle()
    assert h.det.suspended
    h.app.toggle()
    assert not h.det.suspended


def test_auto_stop_ignores_manual_session():
    h = Harness()
    h.app.toggle()
    h.det.on_stop(CallApp.TEAMS)
    assert not h.sessions[0].stopped


def test_auto_start_uses_teams_pid():
    h = Harness()
    h.det.on_start(CallApp.TEAMS)
    assert (h.sessions[0].label, h.sessions[0].exe) == ("Teams", "ms-teams.exe")
    h.det.on_stop(CallApp.TEAMS)
    assert h.sessions[0].stopped


def test_meet_uses_firefox():
    h = Harness()
    h.det.on_start(CallApp.MEET)
    assert h.sessions[0].exe == "firefox.exe"


def test_hotkey_stop_of_auto_session_dismisses():
    h = Harness()
    h.det.on_start(CallApp.TEAMS)
    h.app.toggle()
    assert h.sessions[0].stopped
    assert h.det.dismissed == 1


def test_auto_disabled_suspends_detector():
    h = Harness(auto=False)
    assert h.det.suspended
    assert h.app.state == "idle"
    h.app.set_auto(True)
    assert not h.det.suspended
    assert h.app.state == "armed"


def test_quit_stops_session_and_saves():
    h = Harness()
    h.app.toggle()
    h.app.quit()
    assert h.sessions[0].stopped
    assert not h.det.running


def test_model_wait_does_not_hold_lock():
    import threading

    h = Harness()
    free = []

    def ready(timeout):
        got = []
        t = threading.Thread(target=lambda: got.append(h.app._lock.acquire(timeout=0.5)) or (got[0] and h.app._lock.release()))
        t.start()
        t.join()
        free.append(got[0])
        return True

    h.app._wait_ready = ready
    h.det.on_start(CallApp.TEAMS)
    assert free == [True]
    assert h.sessions and h.sessions[0].started


def test_no_session_after_quit():
    h = Harness()
    h.app.quit()
    h.det.on_start(CallApp.TEAMS)
    h.app.toggle()
    assert h.sessions == []


def test_note_copied_on_stop():
    h = Harness()
    h.app.toggle()
    h.app.toggle()
    assert h.copied == ["texto de Yo"]


def test_call_not_copied():
    h = Harness()
    h.det.on_start(CallApp.TEAMS)
    h.app.toggle()
    assert h.copied == []


def test_empty_note_not_copied():
    h = Harness(text="")
    h.app.toggle()
    h.app.toggle()
    assert h.copied == []
    assert h.sessions[0].stopped


def test_copy_disabled_setting():
    h = Harness(copy_on_stop=False)
    h.app.toggle()
    h.app.toggle()
    assert h.copied == []


def test_session_end_event():
    h = Harness()
    h.app.toggle()
    h.app.toggle()
    assert h.ended == [(Path("Nota.md"), "Nota", True)]


def test_manual_label_is_nota():
    h = Harness()
    h.app.toggle()
    assert (h.sessions[0].label, h.sessions[0].exe) == ("Nota", None)


def test_apply_setting_rejected_while_recording():
    h = Harness()
    h.app.toggle()
    for key, val in [("model", "small"), ("output_dir", "C:/x"), ("language", "en")]:
        assert h.app.apply_setting(key, val) == "No se puede cambiar mientras se graba"
    assert h.app.settings.model == "auto"


def test_apply_hotkey_calls_rebind():
    h = Harness()
    calls = []
    h.app.appliers["hotkey"] = lambda v: calls.append(v)
    assert h.app.apply_setting("hotkey", "<ctrl>+<alt>+n") is None
    assert calls == ["<ctrl>+<alt>+n"] and h.app.settings.hotkey == "<ctrl>+<alt>+n"


def test_apply_hotkey_error_keeps_old():
    h = Harness()

    def boom(v):
        raise ValueError("Esa combinación ya la usa otra aplicación")

    h.app.appliers["hotkey"] = boom
    assert h.app.apply_setting("hotkey", "<ctrl>+<alt>+n") == "Esa combinación ya la usa otra aplicación"
    assert h.app.settings.hotkey == "<ctrl>+<alt>+r"


def test_apply_unknown_key():
    h = Harness()
    assert "desconocido" in h.app.apply_setting("nope", 1)


def test_apply_auto_mode_and_saves():
    h = Harness()
    saved = []
    h.app.save_settings = lambda: saved.append(True)
    assert h.app.apply_setting("auto_mode", False) is None
    assert h.det.suspended and h.app.state == "idle" and saved


def test_stopping_event_before_stop():
    h = Harness()
    seen = []
    h.app.on_session_stopping.append(lambda label: seen.append((label, h.sessions[0].stopped)))
    h.app.toggle()
    h.app.toggle()
    assert seen == [("Nota", False)]


def test_after_apply_runs_with_new_value():
    h = Harness()
    seen = []
    h.app.after_apply["model"] = lambda v: seen.append(h.app.settings.model)
    assert h.app.apply_setting("model", "small") is None
    assert seen == ["small"]


def test_setting_blocked_while_stopping():
    h = Harness()
    h.app.toggle()
    seen = []
    h.app.on_session_stopping.append(lambda label: seen.append(h.app.apply_setting("model", "small")))
    h.app.toggle()
    assert seen == ["No se puede cambiar mientras se graba"]
    assert h.app.busy is False

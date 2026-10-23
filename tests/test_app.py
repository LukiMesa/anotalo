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
    def __init__(self, label, exe):
        self.label, self.exe = label, exe
        self.started = self.stopped = False

    def start(self):
        self.started = True

    def stop(self):
        self.stopped = True
        return Path(f"{self.label}.md")

    def text(self):
        return ""


class Harness:
    def __init__(self, auto=True):
        self.sessions = []
        self.notes = []

        def sf(label, exe, on_line):
            s = FakeSession(label, exe)
            self.sessions.append(s)
            return s

        def df(on_start, on_stop):
            self.det = FakeDetector(on_start, on_stop)
            return self.det

        self.app = App(Settings(auto_mode=auto), df, sf, lambda *a: self.notes.append(a))
        self.app.start()


def test_toggle_starts_and_stops():
    h = Harness()
    h.app.toggle()
    assert h.app.state == "recording"
    s = h.sessions[0]
    assert s.started and s.label == "Manual"
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

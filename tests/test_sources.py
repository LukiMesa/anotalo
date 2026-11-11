from types import SimpleNamespace

import pytest

from calltx.audio import sources
from calltx.audio.sources import find_root_pid


def proc(pid, name, ppid):
    return SimpleNamespace(info={"pid": pid, "name": name, "ppid": ppid})


def test_find_root_pid(monkeypatch):
    procs = [
        proc(10, "explorer.exe", 1),
        proc(100, "firefox.exe", 10),
        proc(101, "firefox.exe", 100),
        proc(102, "firefox.exe", 100),
        proc(103, "firefox.exe", 100),
    ]
    monkeypatch.setattr(sources.psutil, "process_iter", lambda attrs=None: iter(procs))
    assert find_root_pid("firefox.exe") == 100
    assert find_root_pid("ms-teams.exe") is None


from calltx.app import OthersSpec
from calltx.audio.coreaudio import Snapshot
from calltx.audio.routing import SessionInfo

TEAMS = "MSTeams_8wekyb3d8bbwe"


class Fake:
    def __init__(self, *args):
        self.args = args

    def probe(self):
        return None


class Broken(Fake):
    def probe(self):
        raise OSError("activation failed")


def fakes(monkeypatch, process=Fake):
    monkeypatch.setattr(sources, "ProcessLoopbackSource", process)
    monkeypatch.setattr(sources, "EndpointLoopbackSource", type("Endpoint", (Fake,), {}))
    monkeypatch.setattr(sources, "DefaultLoopbackSource", type("Default", (Fake,), {}))
    monkeypatch.setattr(sources, "MicSource", type("Mic", (Fake,), {}))
    monkeypatch.setattr(sources, "find_root_pid", lambda exe: 42)


def render(*sessions, comm="spk", console="spk"):
    names = {"spk": "Altavoces", "bt": "Auriculares (manos libres)"}
    return lambda: Snapshot(list(sessions), names, comm, console)


def test_teams_others_follow_its_output_device(monkeypatch):
    fakes(monkeypatch)
    snap = render(SessionInfo("bt", "Auriculares (manos libres)", 7, True, TEAMS))
    src = sources.pick_others(OthersSpec("device", platform="teams"), render_snapshot=snap)
    assert type(src).__name__ == "Endpoint" and src.args == ("bt", "Auriculares (manos libres)")


def test_process_audio_when_available_else_the_apps_device(monkeypatch):
    fakes(monkeypatch)
    src = sources.pick_others(OthersSpec("process", "zoom.exe", "zoom"), render_snapshot=render())
    assert type(src).__name__ == "Fake" and src.args == (42,)
    fakes(monkeypatch, process=Broken)
    src = sources.pick_others(OthersSpec("process", "zoom.exe", "zoom"), render_snapshot=render())
    assert type(src).__name__ == "Endpoint" and src.args[0] == "spk"


def test_others_fall_back_to_portaudio_default_if_core_audio_fails(monkeypatch):
    fakes(monkeypatch)

    def broken():
        raise OSError("sin Core Audio")

    assert type(sources.pick_others(OthersSpec("device"), render_snapshot=broken)).__name__ == "Default"


def test_mic_is_the_one_the_call_app_uses_unless_set(monkeypatch):
    fakes(monkeypatch)
    cap = lambda: Snapshot([SessionInfo("bt-mic", "Auriculares (micro)", 7, True, TEAMS)],
                           {"bt-mic": "Auriculares (micro)", "int": "Micrófono interno"}, "int", "int")
    assert sources.pick_mic("teams", None, capture_snapshot=cap).args == ("Auriculares (micro)",)
    assert sources.pick_mic("teams", "Elgato", capture_snapshot=cap).args == ("Elgato",)
    none = lambda: Snapshot([], {"int": "Micrófono interno"}, "int", "int")
    assert sources.pick_mic(None, None, capture_snapshot=none).args == ("Micrófono interno",)

    def broken():
        raise OSError("sin Core Audio")

    assert sources.pick_mic("teams", None, capture_snapshot=broken).args == (None,)

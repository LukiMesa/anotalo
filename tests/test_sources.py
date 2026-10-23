from types import SimpleNamespace

import pytest

from calltx.audio import sources
from calltx.audio.sources import find_root_pid, open_call_audio


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


def test_open_call_audio_falls_back(monkeypatch):
    class Broken:
        name = "process"

        def __init__(self, pid):
            pass

        def probe(self):
            raise OSError("activation failed")

    class Default:
        name = "default"

    monkeypatch.setattr(sources, "ProcessLoopbackSource", Broken)
    monkeypatch.setattr(sources, "DefaultLoopbackSource", Default)
    assert isinstance(open_call_audio(1234), Default)
    assert isinstance(open_call_audio(None), Default)


def test_open_call_audio_uses_process(monkeypatch):
    class Ok:
        name = "process"

        def __init__(self, pid):
            self.pid = pid

        def probe(self):
            return None

    monkeypatch.setattr(sources, "ProcessLoopbackSource", Ok)
    src = open_call_audio(42)
    assert isinstance(src, Ok) and src.pid == 42

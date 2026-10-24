import threading
import time

import numpy as np
import pytest

from calltx.asr_remote import RemoteTranscriber
from calltx.config import Settings
from calltx.device import Resolved
from calltx.segmenter import Segment

GPU = Resolved("cuda", "float16", "large-v3-turbo", needs_cuda=False)
CPU = Resolved("cpu", "int8", "small", needs_cuda=False)


def seg(marker=0.5, speaker="Yo"):
    a = np.zeros(1600, np.float32)
    a[0] = marker
    return Segment(speaker, 0.0, 0.1, a)


class Collector:
    def __init__(self):
        self.results = []
        self.lock = threading.Lock()

    def __call__(self, s, text):
        with self.lock:
            self.results.append(text)


def wait_for(cond, timeout=20.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if cond():
            return True
        time.sleep(0.05)
    return False


def make(factory="fake_asr:ok", resolved=CPU, idle_s=None, busy=lambda: False):
    return RemoteTranscriber(Settings(), resolved, idle_s=idle_s, busy=busy, factory=factory, poll_s=0.05)


@pytest.fixture
def remote():
    made = []

    def _make(**kw):
        t = make(**kw)
        made.append(t)
        return t

    yield _make
    for t in made:
        t.stop()


def test_result_delivered_and_hallucination_filtered_in_order(remote):
    t = remote()
    progress = []
    t.start(on_progress=lambda p, m: progress.append(m))
    assert t.loaded and progress == ["Descargando modelo"]
    c = Collector()
    for m in (0.5, 0.6, 0.7, 0.8):
        t.submit(seg(m), c)
    assert t.wait_idle(20)
    assert c.results == ["hola", "adiós", "otra"]


def test_releases_after_idle_and_reloads_on_submit(remote):
    t = remote(idle_s=0.3)
    t.start()
    c = Collector()
    t.submit(seg(), c)
    assert t.wait_idle(20)
    assert wait_for(lambda: not t.loaded, 10)
    t.submit(seg(0.7), c)
    assert t.wait_idle(30)
    assert c.results == ["hola", "adiós"]


def test_busy_prevents_release(remote):
    busy = {"v": True}
    t = remote(idle_s=0.2, busy=lambda: busy["v"])
    t.start()
    time.sleep(0.8)
    assert t.loaded
    busy["v"] = False
    assert wait_for(lambda: not t.loaded, 10)


def test_never_release_when_idle_is_none(remote):
    t = remote(idle_s=None)
    t.start()
    time.sleep(0.5)
    assert t.loaded


def test_idle_can_change_live(remote):
    t = remote(idle_s=None)
    t.start()
    t.idle_s = 0.2
    assert wait_for(lambda: not t.loaded, 10)


def test_warm_starts_child_in_background(remote):
    t = remote(idle_s=0.2)
    t.start()
    assert wait_for(lambda: not t.loaded, 10)
    t.warm()
    assert wait_for(lambda: t.loaded, 30)


def test_child_crash_resends_pending_once(remote, tmp_path, monkeypatch):
    monkeypatch.setenv("CALLTX_TEST_CRASH", str(tmp_path / "crashed"))
    t = remote()
    t.start()
    c = Collector()
    t.submit(seg(0.9), c)
    t.submit(seg(0.7), c)
    assert t.wait_idle(40)
    assert c.results == ["hola", "adiós"]
    assert (tmp_path / "crashed").exists()


def test_cpu_fallback_is_kept_for_relaunch(remote, tmp_path, monkeypatch):
    log = tmp_path / "devices.txt"
    monkeypatch.setenv("CALLTX_TEST_DEVICES", str(log))
    t = remote(factory="fake_asr:gpu_fails", resolved=GPU, idle_s=0.2)
    t.start()
    assert t.device == "cpu" and t.fallback_used and t.resolved.device == "cpu"
    assert wait_for(lambda: not t.loaded, 10)
    c = Collector()
    t.submit(seg(), c)
    assert t.wait_idle(30) and c.results == ["hola"]
    assert log.read_text().split() == ["cuda", "cpu", "cpu"]


def test_load_failure_raises(remote):
    t = remote(factory="fake_asr:always_fails")
    with pytest.raises(RuntimeError, match="no carga"):
        t.start()
    assert not t.loaded


def test_stop_without_child_returns_quickly():
    t = make(idle_s=0.1)
    t.start()
    assert wait_for(lambda: not t.loaded, 10)
    t0 = time.monotonic()
    t.stop()
    assert time.monotonic() - t0 < 2


def test_wait_idle_includes_segments_queued_while_starting(remote):
    t = remote(idle_s=0.2)
    t.start()
    assert wait_for(lambda: not t.loaded, 10)
    c = Collector()
    t.submit(seg(), c)  # el hijo está cerrado: se encola y se relanza
    assert not t.loaded
    assert t.wait_idle(30)
    assert c.results == ["hola"]


def test_notifies_loaded_changes(remote):
    seen = []
    t = remote(idle_s=0.2)
    t._on_change = lambda: seen.append(t.loaded)
    t.start()
    assert wait_for(lambda: False in seen, 10)
    assert seen[0] is True and seen[-1] is False


def test_selftest_roundtrip_spawns_real_child():
    from calltx.asr_remote import selftest_roundtrip

    assert selftest_roundtrip() == "ok"

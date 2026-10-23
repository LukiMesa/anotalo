import threading

from calltx.models import ModelHolder


class FakeT:
    def __init__(self, name, fail=False):
        self.name, self.fail, self.stopped = name, fail, False

    def start(self):
        if self.fail:
            raise RuntimeError("no carga")

    def stop(self):
        self.stopped = True


def test_initial_load_sets_ready():
    h = ModelHolder(lambda: FakeT("a"), busy=lambda: False)
    h.reload_sync()
    assert h.ready.is_set() and h.current.name == "a"


def test_reload_failure_keeps_old_and_ready():
    names = iter([FakeT("a"), FakeT("b", fail=True)])
    h = ModelHolder(lambda: next(names), busy=lambda: False)
    h.reload_sync()
    assert h.reload_sync() is False
    assert h.current.name == "a" and h.ready.is_set() and not h.current.stopped


def test_old_not_stopped_while_busy_then_stopped_on_idle():
    busy = {"v": True}
    names = iter([FakeT("a"), FakeT("b")])
    h = ModelHolder(lambda: next(names), busy=lambda: busy["v"])
    h.reload_sync()
    old = h.current
    h.reload_sync()
    assert h.current.name == "b" and not old.stopped
    busy["v"] = False
    h.on_idle()
    assert old.stopped


def test_concurrent_reloads_serialized():
    made = []
    gate = threading.Event()

    def factory():
        t = FakeT(f"m{len(made)}")
        made.append(t)
        return t

    h = ModelHolder(factory, busy=lambda: False)
    ts = [threading.Thread(target=h.reload_sync) for _ in range(3)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert h.current is made[-1]
    assert all(m.stopped for m in made[:-1])


def test_holder_reports_progress():
    class ProgT(FakeT):
        def start(self, on_progress=None):
            on_progress(0.5, "Descargando modelo")

    seen = []
    h = ModelHolder(lambda: ProgT("a"), busy=lambda: False)
    h.reload_sync(on_progress=lambda p, m: seen.append((p, m)))
    assert seen == [(0.5, "Descargando modelo")]

import pytest

from calltx.ui.winchrome import EDGES, Chrome


class FakeWin32:
    def __init__(self):
        self.rect = (100, 100, 1220, 820)
        self.work = (0, 0, 1920, 1040)  # sin la barra de tareas
        self.calls = []

    def get_rect(self, hwnd):
        return self.rect

    def set_rect(self, hwnd, rect):
        self.rect = rect
        self.calls.append(("set_rect", rect))

    def work_area(self, hwnd):
        return self.work

    def send_nc(self, hwnd, hit):
        self.calls.append(("nc", hit))

    def minimize(self, hwnd):
        self.calls.append(("min",))


def make():
    w = FakeWin32()
    return Chrome(lambda: 42, w), w


def test_maximize_rect_uses_monitor_work_area():
    c, w = make()
    assert c.toggle_maximize() is True
    assert w.rect == (0, 0, 1920, 1040)


def test_toggle_restores_previous_rect():
    c, w = make()
    c.toggle_maximize()
    assert c.toggle_maximize() is False
    assert w.rect == (100, 100, 1220, 820)


def test_drag_and_resize_send_native_hits():
    c, w = make()
    c.start_drag()
    c.start_resize("bottomright")
    assert w.calls == [("nc", 2), ("nc", EDGES["bottomright"])]


def test_resize_edge_validated():
    c, _ = make()
    with pytest.raises(ValueError):
        c.start_resize("diagonal")


def test_drag_from_maximized_restores_first():
    c, w = make()
    c.toggle_maximize()
    c.start_drag()
    assert c.maximized is False


def test_resize_bounds_edges_and_min_size():
    from calltx.ui.window import resize_bounds

    start = (100, 100, 1000, 700)
    assert resize_bounds("bottomright", start, 50, 30, 940, 600) == (100, 100, 1050, 730)
    assert resize_bounds("left", start, 40, 0, 940, 600) == (140, 100, 960, 700)
    assert resize_bounds("left", start, 200, 0, 940, 600) == (160, 100, 940, 700)  # no baja del mínimo
    assert resize_bounds("top", start, 0, -20, 940, 600) == (100, 80, 1000, 720)


def test_adopt_native_maximize_then_restore():
    c, w = make()
    events = []
    c._on_maximized = events.append
    c.adopt_maximized((10, 20, 900, 700))
    assert c.maximized is True and events == [True]
    assert c.toggle_maximize() is False
    assert w.rect == (10, 20, 900, 700)

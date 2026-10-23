from calltx.ui.winchrome import Chrome


class FakeWin32:
    def __init__(self):
        self.zoomed = False
        self.calls = []

    def is_zoomed(self, hwnd):
        return self.zoomed

    def maximize(self, hwnd):
        self.zoomed = True
        self.calls.append("max")

    def restore(self, hwnd):
        self.zoomed = False
        self.calls.append("restore")

    def minimize(self, hwnd):
        self.calls.append("min")


def make():
    w = FakeWin32()
    return Chrome(lambda: 42, w), w


def test_toggle_uses_native_maximize_and_restore():
    c, w = make()
    assert c.toggle_maximize() is True and w.calls == ["max"]
    assert c.toggle_maximize() is False and w.calls == ["max", "restore"]


def test_maximized_reflects_window_state():
    # p. ej. maximizada con doble clic o Win+↑: el estado se lee de Windows
    c, w = make()
    w.zoomed = True
    assert c.maximized is True
    assert c.toggle_maximize() is False and w.calls == ["restore"]


def test_minimize_and_hide():
    hidden = []
    w = FakeWin32()
    c = Chrome(lambda: 42, w, hide=lambda: hidden.append(1))
    c.minimize()
    c.hide()
    assert w.calls == ["min"] and hidden == [1]


def test_work_area_relative_to_monitor():
    from calltx.ui.window import maximized_bounds

    # monitor secundario a la derecha, barra de tareas abajo de 48 px
    assert maximized_bounds(screen=(2560, 0, 1920, 1080), work=(2560, 0, 1920, 1032)) == (0, 0, 1920, 1032)
    # barra de tareas arriba en el principal
    assert maximized_bounds(screen=(0, 0, 2560, 1440), work=(0, 48, 2560, 1392)) == (0, 48, 2560, 1392)


def test_resize_bounds_edges_and_min_size():
    from calltx.ui.window import resize_bounds

    start = (100, 100, 1000, 700)
    assert resize_bounds("bottomright", start, 50, 30, 940, 600) == (100, 100, 1050, 730)
    assert resize_bounds("left", start, 40, 0, 940, 600) == (140, 100, 960, 700)
    assert resize_bounds("left", start, 200, 0, 940, 600) == (160, 100, 940, 700)
    assert resize_bounds("top", start, 0, -20, 940, 600) == (100, 80, 1000, 720)


def test_main_window_exposes_hwnd_for_chrome():
    # main.py conecta Chrome con MainWindow.get_hwnd: si falta, los botones de la barra no hacen nada
    from calltx.ui.window import MainWindow

    w = object.__new__(MainWindow)
    w.hwnd = 1234
    assert w.get_hwnd() == 1234

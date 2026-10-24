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


def test_main_window_exposes_hwnd_for_chrome():
    # main.py conecta Chrome con MainWindow.get_hwnd: si falta, los botones de la barra no hacen nada
    from calltx.ui.window import MainWindow

    w = object.__new__(MainWindow)
    w.hwnd = 1234
    assert w.get_hwnd() == 1234

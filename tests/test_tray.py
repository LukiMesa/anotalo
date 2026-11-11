import pystray

from calltx.ui.tray import Tray


class FakeApp:
    def __init__(self):
        self.state = "armed"
        self.session_kind = None
        self.on_state_change = []
        self.calls = []
        self.settings = type("S", (), {"auto_mode": True})()

    def toggle(self):
        self.calls.append("toggle")

    def toggle_call(self):
        self.calls.append("call")


def _items(tray):
    return [i for i in tray._icon.menu.items if i is not pystray.Menu.SEPARATOR]


def test_tray_has_record_call_item():
    app = FakeApp()
    tray = Tray(app, open_window=lambda: None, open_folder=lambda: None, on_quit=lambda: None)
    item = next(i for i in _items(tray) if i.text == "Grabar llamada")
    assert item.visible
    item(tray._icon)
    import time
    time.sleep(0.1)
    assert app.calls == ["call"]
    app.state, app.session_kind = "recording", "manual"
    assert not item.visible  # mientras graba, "Detener grabación" ya lo para

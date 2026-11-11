import sys
from types import SimpleNamespace

from calltx.audio import pa


class FakeStream:
    def start_stream(self):
        pass

    def stop_stream(self):
        pass

    def close(self):
        pass

    def is_active(self):
        return True


class FakePA:
    created: list = []

    def __init__(self):
        self.terminated = False
        FakePA.created.append(self)

    def terminate(self):
        self.terminated = True

    def get_host_api_info_by_type(self, _t):
        return {"index": 0, "defaultInputDevice": 0}

    def get_device_count(self):
        return 1

    def get_device_info_by_index(self, _i):
        return {"index": 0, "name": "Micrófono", "hostApi": 0, "maxInputChannels": 1, "defaultSampleRate": 16000}

    def open(self, **_kw):
        return FakeStream()


def test_device_list_is_read_fresh_for_each_recording(monkeypatch):
    # Auriculares conectados después de abrir la app: la lista de dispositivos se lee de nuevo
    FakePA.created = []
    monkeypatch.setitem(sys.modules, "pyaudiowpatch",
                        SimpleNamespace(PyAudio=FakePA, paWASAPI=13, paInt16=8, paContinue=0))
    monkeypatch.setattr(pa, "_pa", None)
    monkeypatch.setattr(pa, "_open", 0)
    first = pa.MicSource()
    first.start(lambda *a: None)
    pa.MicSource()._device()  # con una grabación abierta no se toca la lista
    assert len(FakePA.created) == 1
    first.stop()
    pa.MicSource()._device()  # nada abierto: lista nueva
    assert len(FakePA.created) == 2 and FakePA.created[0].terminated

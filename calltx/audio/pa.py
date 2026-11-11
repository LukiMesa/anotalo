"""Micrófono y loopback del dispositivo por defecto vía PyAudioWPatch (WASAPI).

PortAudio lee la lista de dispositivos una sola vez: se vuelve a leer al empezar cada grabación
(si no hay nada abierto), para ver los auriculares conectados después de abrir la app.
"""

from __future__ import annotations

import logging
import threading
import time

import numpy as np

from .base import OnAudio
from .resample import to_mono_16k

log = logging.getLogger(__name__)

_pa_lock = threading.Lock()
_pa = None
_open = 0  # flujos abiertos ahora


def _pyaudio(refresh: bool = False):
    global _pa
    import pyaudiowpatch as pyaudio

    with _pa_lock:
        if refresh and _pa is not None and _open == 0:
            try:
                _pa.terminate()
            except Exception:  # noqa: BLE001
                log.debug("No se pudo cerrar PortAudio", exc_info=True)
            _pa = None
        if _pa is None:
            _pa = pyaudio.PyAudio()
        return pyaudio, _pa


def _opened(delta: int) -> None:
    global _open
    with _pa_lock:
        _open = max(0, _open + delta)


class _PaSource:
    name = "pa"

    def __init__(self):
        self._stream = None

    def _device(self) -> dict:
        raise NotImplementedError

    def start(self, on_audio: OnAudio) -> None:
        dev = self._device()  # puede volver a leer la lista de dispositivos
        pyaudio, pa = _pyaudio()
        rate = int(dev["defaultSampleRate"])
        channels = max(1, min(2, int(dev["maxInputChannels"])))
        log.info("%s: %s (%d Hz, %d ch)", self.name, dev["name"], rate, channels)

        def cb(in_data, frame_count, _time_info, _status):
            now = time.monotonic()
            x = np.frombuffer(in_data, dtype=np.int16)
            on_audio(now - frame_count / rate, to_mono_16k(x, rate, channels))
            return (None, pyaudio.paContinue)

        self._stream = pa.open(
            format=pyaudio.paInt16,
            channels=channels,
            rate=rate,
            input=True,
            input_device_index=dev["index"],
            frames_per_buffer=int(rate * 0.05),
            stream_callback=cb,
        )
        _opened(+1)
        self._stream.start_stream()

    def stop(self) -> None:
        s, self._stream = self._stream, None
        if s is not None:
            try:
                s.stop_stream()
                s.close()
            except Exception:  # noqa: BLE001
                log.exception("Error cerrando %s", self.name)
            finally:
                _opened(-1)

    def is_active(self) -> bool:
        return self._stream is not None and self._stream.is_active()


class MicSource(_PaSource):
    name = "mic"

    def __init__(self, device_name: str | None = None):
        super().__init__()
        self._want = device_name

    def _device(self) -> dict:
        pyaudio, pa = _pyaudio(refresh=True)
        wasapi = pa.get_host_api_info_by_type(pyaudio.paWASAPI)
        if self._want:
            for i in range(pa.get_device_count()):
                d = pa.get_device_info_by_index(i)
                if (
                    d["hostApi"] == wasapi["index"]
                    and d["maxInputChannels"] > 0
                    and not d.get("isLoopbackDevice")
                    and self._want.lower() in d["name"].lower()
                ):
                    return d
            log.warning("Micrófono '%s' no encontrado; uso el predeterminado", self._want)
        return pa.get_device_info_by_index(wasapi["defaultInputDevice"])


class DefaultLoopbackSource(_PaSource):
    name = "loopback-default"

    def _device(self) -> dict:
        _, pa = _pyaudio(refresh=True)
        return pa.get_default_wasapi_loopback()

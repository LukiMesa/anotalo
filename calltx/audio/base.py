from __future__ import annotations

from typing import Callable, Protocol

import numpy as np

# Recibe (instante monotónico de la primera muestra, muestras float32 mono 16 kHz)
OnAudio = Callable[[float, np.ndarray], None]


class AudioSource(Protocol):
    name: str

    def start(self, on_audio: OnAudio) -> None: ...

    def stop(self) -> None: ...

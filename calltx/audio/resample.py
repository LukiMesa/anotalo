"""Conversión de cualquier formato de captura a float32 mono 16 kHz."""

from __future__ import annotations

import numpy as np
import soxr

TARGET_SR = 16000


def to_mono_16k(samples: np.ndarray, sample_rate: int, channels: int) -> np.ndarray:
    x = np.asarray(samples)
    if x.dtype == np.int16:
        x = x.astype(np.float32) / 32768.0
    elif x.dtype == np.int32:
        x = x.astype(np.float32) / 2147483648.0
    else:
        x = x.astype(np.float32, copy=False)
    if channels > 1:
        x = x[: len(x) - len(x) % channels].reshape(-1, channels).mean(axis=1)
    if sample_rate != TARGET_SR:
        x = soxr.resample(x, sample_rate, TARGET_SR, quality="HQ")
    return np.ascontiguousarray(x, dtype=np.float32)

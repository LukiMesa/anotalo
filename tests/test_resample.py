import numpy as np

from calltx.audio.resample import to_mono_16k


def test_resample_48k_stereo_to_16k_mono_length():
    stereo = np.zeros(48000 * 2, dtype=np.float32)
    out = to_mono_16k(stereo, 48000, 2)
    assert out.dtype == np.float32
    assert abs(len(out) - 16000) <= 1


def test_resample_int16_scaled():
    x = np.full(1600, 32767, dtype=np.int16)
    out = to_mono_16k(x, 16000, 1)
    assert 0.99 < out.max() <= 1.0


def test_stereo_averaged():
    x = np.array([1.0, 0.0] * 800, dtype=np.float32)
    out = to_mono_16k(x, 16000, 2)
    assert np.allclose(out, 0.5)

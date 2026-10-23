import numpy as np
import pytest

from calltx.segmenter import Segmenter

SR = 16000


def fake_vad(frame: np.ndarray) -> float:
    return 1.0 if np.sqrt(np.mean(frame**2)) > 0.01 else 0.0


def tone(seconds: float) -> np.ndarray:
    t = np.arange(int(seconds * SR)) / SR
    return (0.3 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)


def silence(seconds: float) -> np.ndarray:
    return np.zeros(int(seconds * SR), dtype=np.float32)


def run(chunks, **kw):
    """chunks: list of (t_s, samples)."""
    out = []
    seg = Segmenter("Yo", fake_vad, out.append, **kw)
    for t, x in chunks:
        # feed in 10 ms pieces, like a real capture callback
        for i in range(0, len(x), 160):
            seg.feed(t + i / SR, x[i : i + 160])
    seg.flush()
    return out


def test_single_utterance():
    x = np.concatenate([silence(0.5), tone(1.0), silence(1.0)])
    segs = run([(0.0, x)])
    assert len(segs) == 1
    s = segs[0]
    assert s.speaker == "Yo"
    assert abs(s.start_s - 0.5) < 0.25
    assert abs(s.end_s - 1.5) < 0.7
    assert len(s.audio) == pytest.approx((s.end_s - s.start_s) * SR, abs=SR * 0.05)


def test_short_blip_dropped():
    x = np.concatenate([silence(0.5), tone(0.1), silence(1.0)])
    assert run([(0.0, x)]) == []


def test_force_cut():
    segs = run([(0.0, tone(30.0))], max_segment_s=25.0)
    assert len(segs) == 2
    assert segs[0].end_s - segs[0].start_s <= 25.0 + 0.05


def test_gap_counts_as_silence():
    segs = run([(0.0, tone(1.0)), (5.0, tone(1.0))])
    assert len(segs) == 2
    assert abs(segs[1].start_s - 5.0) < 0.25
    assert segs[0].end_s < 2.0


def test_flush_emits_open():
    out = []
    seg = Segmenter("Otros", fake_vad, out.append)
    x = tone(1.0)
    for i in range(0, len(x), 160):
        seg.feed(i / SR, x[i : i + 160])
    assert out == []
    seg.flush()
    assert len(out) == 1 and out[0].speaker == "Otros"


def test_silero_loads():
    from calltx.vad import SileroVad

    v = SileroVad()
    assert v.prob(np.zeros(512, np.float32)) < 0.5


def test_callback_jitter_does_not_split_or_shift():
    # Callbacks que llegan 80 ms tarde no son huecos: una sola frase y tiempos correctos.
    out = []
    seg = Segmenter("Yo", fake_vad, out.append)
    x = np.concatenate([silence(0.5), tone(3.0), silence(1.0)])
    for n, i in enumerate(range(0, len(x), 800)):
        jitter = 0.08 if n % 2 else 0.0
        seg.feed(i / SR + jitter, x[i : i + 800])
    seg.flush()
    assert len(out) == 1
    assert abs(out[0].start_s - 0.5) < 0.25
    assert abs(out[0].end_s - 3.5) < 0.5
    # 3 s de voz + ~0.2 s de pre-roll + ~0.2 s de margen final
    assert abs(len(out[0].audio) / SR - 3.4) < 0.1


def test_idle_finishes_open_segment():
    out = []
    seg = Segmenter("Otros", fake_vad, out.append)
    x = tone(1.0)
    for i in range(0, len(x), 160):
        seg.feed(i / SR, x[i : i + 160])
    seg.idle(3.0)
    assert len(out) == 1


def test_jitter_does_not_ratchet_later_timestamps():
    out = []
    seg = Segmenter("Yo", fake_vad, out.append)
    x = np.concatenate([silence(0.5), tone(3.0), silence(2.5), tone(1.0), silence(1.0)])
    for n, i in enumerate(range(0, len(x), 800)):
        seg.feed(i / SR + (0.08 if n % 2 else 0.0), x[i : i + 800])
    seg.flush()
    assert len(out) == 2
    assert abs(out[1].start_s - 6.0) < 0.3

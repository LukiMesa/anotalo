import threading
from types import SimpleNamespace

import numpy as np

from calltx.asr import Transcriber, is_hallucination
from calltx.config import Settings
from calltx.segmenter import Segment


class FakeModel:
    def __init__(self, texts=None, logprob=-0.2, nsp=0.05):
        self.texts = texts or {}
        self.logprob = logprob
        self.nsp = nsp
        self.calls = 0

    def transcribe(self, audio, **kw):
        self.calls += 1
        key = round(float(audio[0]), 3) if len(audio) else 0.0
        text = self.texts.get(key, "hola")
        return iter([SimpleNamespace(text=" " + text, avg_logprob=self.logprob, no_speech_prob=self.nsp)]), None


def seg(marker=0.5, speaker="Yo"):
    a = np.zeros(16000, np.float32)
    a[0] = marker
    return Segment(speaker, 0.0, 1.0, a)


def collect(t, segs):
    results, done = [], threading.Event()
    n = len(segs)

    def cb(s, text):
        results.append((s.speaker, text))
        if len(results) == n:
            done.set()

    for s in segs:
        t.submit(s, cb)
    t.stop()
    return results


def make(model):
    t = Transcriber(Settings(), model_factory=lambda **kw: model)
    t.start()
    return t


def test_result_delivered():
    assert collect(make(FakeModel()), [seg()]) == [("Yo", "hola")]


def test_hallucination_filtered():
    m = FakeModel(texts={0.5: "Gracias por ver"})
    assert collect(make(m), [seg()]) == []


def test_low_confidence_filtered():
    assert collect(make(FakeModel(logprob=-1.5, nsp=0.8)), [seg()]) == []


def test_fifo_order():
    m = FakeModel(texts={0.1: "uno", 0.2: "dos", 0.3: "tres"})
    out = collect(make(m), [seg(0.1), seg(0.2), seg(0.3)])
    assert [x[1] for x in out] == ["uno", "dos", "tres"]


def test_fallback_on_cuda_error():
    calls = []

    def factory(**kw):
        calls.append(kw)
        if kw["device"] == "cuda":
            raise RuntimeError("CUDA failed")
        return FakeModel()

    t = Transcriber(Settings(), model_factory=factory)
    t.start()
    assert t.fallback_used is True
    assert t.device == "cpu"
    assert calls[-1]["model"] == "small"
    t.stop()


def test_stop_drains():
    m = FakeModel()
    t = make(m)
    out = collect(t, [seg() for _ in range(5)])
    assert len(out) == 5


def test_is_hallucination_rules():
    assert is_hallucination("", -0.1, 0.0)
    assert is_hallucination("...", -0.1, 0.0)
    assert is_hallucination("Subtítulos realizados por la comunidad de Amara.org", -0.1, 0.0)
    assert not is_hallucination("Quiero cambiar el logo", -0.3, 0.1)

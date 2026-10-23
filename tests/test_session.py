import numpy as np

from calltx.config import Settings
from calltx.session import CallSession

SR = 16000
T0 = 100.0


def fake_vad_factory():
    return lambda frame: 1.0 if np.sqrt(np.mean(frame**2)) > 0.01 else 0.0


def tone(seconds):
    t = np.arange(int(seconds * SR)) / SR
    return (0.3 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)


class FakeSource:
    def __init__(self, name, speech_at, fail_times=0):
        self.name = name
        self.speech_at = speech_at
        self.fail_times = fail_times
        self.starts = 0
        self.stopped = False

    def start(self, on_audio):
        self.starts += 1
        if self.fail_times:
            self.fail_times -= 1
            raise OSError("device gone")
        x = np.concatenate([np.zeros(int(self.speech_at * SR), np.float32), tone(1.0), np.zeros(SR, np.float32)])
        for i in range(0, len(x), 800):
            on_audio(T0 + i / SR, x[i : i + 800])

    def stop(self):
        self.stopped = True


class FakeTranscriber:
    def submit(self, seg, cb):
        cb(seg, f"texto de {seg.speaker}")

    def wait_idle(self):
        pass


def make(tmp_path, mic, others, lines=None):
    s = Settings(output_dir=str(tmp_path))
    return CallSession(
        s,
        FakeTranscriber(),
        "Manual",
        mic,
        others,
        on_line=(lambda *a: lines.append(a)) if lines is not None else None,
        vad_factory=fake_vad_factory,
        clock=lambda: T0,
        retry_delay_s=0.0,
    )


def test_two_speakers_in_order(tmp_path):
    lines = []
    sess = make(tmp_path, FakeSource("mic", 1.5), FakeSource("others", 3.5), lines)
    sess.start()
    path = sess.stop()
    text = path.read_text(encoding="utf-8")
    assert "**[00:00:01] Yo:** texto de Yo" in text
    assert "**[00:00:03] Otros:** texto de Otros" in text
    assert text.index("Yo:**") < text.index("Otros:**")
    assert {l[0] for l in lines} == {"Yo", "Otros"}


def test_stop_flushes_pending(tmp_path):
    mic, others = FakeSource("mic", 0.5), FakeSource("others", 0.5)
    sess = make(tmp_path, mic, others)
    sess.start()
    path = sess.stop()
    assert mic.stopped and others.stopped
    text = path.read_text(encoding="utf-8")
    assert "Yo:**" in text and "Otros:**" in text
    assert "- Duración:" in text


def test_source_failure_reopens(tmp_path):
    others = FakeSource("others", 2.0, fail_times=1)
    sess = make(tmp_path, FakeSource("mic", 0.5), others)
    sess.start()
    path = sess.stop()
    assert others.starts == 2
    assert "Otros:**" in path.read_text(encoding="utf-8")

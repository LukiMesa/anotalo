"""Silero VAD en streaming (modelo onnx incluido en faster-whisper)."""

from __future__ import annotations

from importlib import resources

import numpy as np

FRAME = 512
CONTEXT = 64


def _model_path() -> str:
    return str(resources.files("faster_whisper") / "assets" / "silero_vad_v6.onnx")


class SileroVad:
    def __init__(self):
        import onnxruntime

        opts = onnxruntime.SessionOptions()
        opts.inter_op_num_threads = 1
        opts.intra_op_num_threads = 1
        opts.log_severity_level = 4
        self._sess = onnxruntime.InferenceSession(
            _model_path(), providers=["CPUExecutionProvider"], sess_options=opts
        )
        self.reset()

    def reset(self) -> None:
        self._h = np.zeros((1, 1, 128), dtype=np.float32)
        self._c = np.zeros((1, 1, 128), dtype=np.float32)
        self._ctx = np.zeros(CONTEXT, dtype=np.float32)

    def prob(self, frame: np.ndarray) -> float:
        frame = np.asarray(frame, dtype=np.float32)
        x = np.concatenate([self._ctx, frame])[None, :]
        out, self._h, self._c = self._sess.run(None, {"input": x, "h": self._h, "c": self._c})
        self._ctx = frame[-CONTEXT:].copy()
        return float(np.asarray(out).reshape(-1)[0])

    __call__ = prob

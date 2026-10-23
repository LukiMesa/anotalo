import io
import json
import zipfile
from pathlib import Path

import pytest

from calltx.cuda_setup import WHEELS, CudaSetup, CudaSetupError


def make_wheel(pkg: str) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr(f"nvidia/{pkg}/bin/{pkg}64_12.dll", b"DLL" * 100)
        z.writestr(f"nvidia/{pkg}/include/x.h", b"no")
        z.writestr("README.txt", b"no")
    return buf.getvalue()


class FakeNet:
    def __init__(self, fail_on=None):
        self.wheels = {name: make_wheel(name.split("-")[1]) for name, _ in WHEELS}
        self.fail_on = fail_on
        self.resume_offsets = []

    def fetch_json(self, url):
        name = url.split("/pypi/")[1].split("/")[0]
        data = self.wheels[name]
        return {"urls": [
            {"filename": f"{name}-x-py3-none-manylinux.whl", "url": f"https://x/{name}-linux.whl", "size": 1},
            {"filename": f"{name}-x-py3-none-win_amd64.whl", "url": f"https://x/{name}.whl", "size": len(data)},
        ]}

    def download(self, url, dest: Path, progress):
        name = url.rsplit("/", 1)[1][:-4]
        data = self.wheels[name]
        start = dest.stat().st_size if dest.exists() else 0
        self.resume_offsets.append((name, start))
        with open(dest, "ab") as f:
            for i in range(start, len(data), 50):
                if self.fail_on == name and i > len(data) // 2:
                    raise OSError("red caída")
                f.write(data[i:i + 50])
                progress(i + len(data[i:i + 50]), len(data))


def test_run_extracts_dlls_and_marks_ready(tmp_path):
    net = FakeNet()
    s = CudaSetup(tmp_path, net.fetch_json, net.download)
    assert not s.is_ready()
    s.run(lambda p, msg: None)
    assert s.is_ready()
    assert (tmp_path / "cublas" / "cublas64_12.dll").exists()
    assert not list(tmp_path.rglob("*.h"))
    assert not list(tmp_path.rglob("*.whl"))


def test_partial_download_not_ready(tmp_path):
    net = FakeNet(fail_on="nvidia-cudnn-cu12")
    s = CudaSetup(tmp_path, net.fetch_json, net.download)
    with pytest.raises(CudaSetupError):
        s.run(lambda p, msg: None)
    assert not s.is_ready()
    assert not (tmp_path / "ready.json").exists()


def test_resume_uses_existing_part(tmp_path):
    net = FakeNet(fail_on="nvidia-cudnn-cu12")
    s = CudaSetup(tmp_path, net.fetch_json, net.download)
    with pytest.raises(CudaSetupError):
        s.run(lambda p, msg: None)
    net.fail_on = None
    s.run(lambda p, msg: None)
    offsets = [o for n, o in net.resume_offsets if n == "nvidia-cudnn-cu12"]
    assert offsets[0] == 0 and offsets[-1] > 0
    assert s.is_ready()


def test_marker_mismatch_not_ready(tmp_path):
    (tmp_path / "ready.json").write_text(json.dumps({"wheels": [["nvidia-cublas-cu12", "1.0"]]}), encoding="utf-8")
    assert not CudaSetup(tmp_path, None, None).is_ready()


def test_progress_monotonic_to_1(tmp_path):
    net = FakeNet()
    seen = []
    CudaSetup(tmp_path, net.fetch_json, net.download).run(lambda p, msg: seen.append(p))
    assert seen == sorted(seen) and seen[-1] == pytest.approx(1.0)


def test_complete_part_is_not_downloaded_again(tmp_path):
    # Se cerró la app justo tras descargar (antes de extraer): no pedir más bytes (HTTP 416).
    net = FakeNet()
    s = CudaSetup(tmp_path, net.fetch_json, net.download)
    dl = tmp_path / "_dl"
    dl.mkdir(parents=True)
    for name, _ in WHEELS:
        (dl / f"{name}.whl.part").write_bytes(net.wheels[name])

    def no_download(url, dest, progress):
        raise AssertionError("no debía descargar")

    CudaSetup(tmp_path, net.fetch_json, no_download).run(lambda p, m: None)
    assert s.is_ready()


def test_runs_are_serialized(tmp_path):
    import threading
    import time

    net = FakeNet()
    active, overlaps = [0], []

    def slow_download(url, dest, progress):
        active[0] += 1
        if active[0] > 1:
            overlaps.append(url)
        time.sleep(0.05)
        net.download(url, dest, progress)
        active[0] -= 1

    ts = [threading.Thread(target=lambda: CudaSetup(tmp_path, net.fetch_json, slow_download).run(lambda p, m: None))
          for _ in range(2)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert overlaps == []
    assert CudaSetup(tmp_path, None, None).is_ready()

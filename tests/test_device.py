from calltx.config import Settings
from calltx.device import has_nvidia_driver, resolve


def test_auto_with_gpu_ready():
    r = resolve(Settings(), nvidia=True, cuda_ready=True)
    assert (r.device, r.compute_type, r.model, r.needs_cuda) == ("cuda", "float16", "large-v3-turbo", False)


def test_auto_with_gpu_not_ready_needs_cuda():
    r = resolve(Settings(), nvidia=True, cuda_ready=False)
    assert r.device == "cuda" and r.needs_cuda is True


def test_auto_without_gpu_cpu_small():
    r = resolve(Settings(), nvidia=False, cuda_ready=False)
    assert (r.device, r.compute_type, r.model, r.needs_cuda) == ("cpu", "int8", "small", False)


def test_explicit_cpu():
    r = resolve(Settings(device="cpu"), nvidia=True, cuda_ready=True)
    assert r.device == "cpu" and r.model == "small"


def test_explicit_cuda_without_gpu_falls_to_cpu():
    r = resolve(Settings(device="cuda"), nvidia=False, cuda_ready=False)
    assert r.device == "cpu"


def test_explicit_model_kept_on_cpu():
    r = resolve(Settings(model="medium"), nvidia=False, cuda_ready=False)
    assert r.model == "medium" and r.device == "cpu"


def test_has_nvidia_driver_false_when_dll_missing():
    def loader(name):
        raise OSError("no está")

    assert has_nvidia_driver(loader) is False
    assert has_nvidia_driver(lambda name: object()) is True

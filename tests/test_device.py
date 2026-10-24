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

    class Dll:
        def __init__(self, v):
            self.v = v

        def cuDriverGetVersion(self, ref):
            ref._obj.value = self.v
            return 0

    assert has_nvidia_driver(lambda name: Dll(12080)) is True
    assert has_nvidia_driver(lambda name: Dll(11040)) is False


def test_nvidia_status_versions():
    from calltx.device import nvidia_status

    assert nvidia_status(lambda: None) == "none"
    assert nvidia_status(lambda: 11080) == "old"
    assert nvidia_status(lambda: 12000) == "ok"


def test_driver_version_force_cpu(monkeypatch):
    from calltx.device import driver_cuda_version

    monkeypatch.setenv("CALLTX_FORCE_CPU", "1")

    def loader(name):
        raise AssertionError("no debe cargar")

    assert driver_cuda_version(loader=loader) is None


def test_resolve_old_driver_uses_cpu_even_if_cuda_ready():
    from calltx.config import Settings
    from calltx.device import nvidia_status, resolve

    r = resolve(Settings(), nvidia_status(lambda: 11080) == "ok", cuda_ready=True)
    assert r.device == "cpu"

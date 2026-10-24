"""Elige GPU o procesador y el modelo adecuado para este PC."""

from __future__ import annotations

import ctypes
import os
from dataclasses import dataclass
from typing import Callable

GPU_MODEL = "large-v3-turbo"
CPU_MODEL = "small"


@dataclass(frozen=True)
class Resolved:
    device: str  # "cuda" | "cpu"
    compute_type: str
    model: str
    needs_cuda: bool  # hay que descargar los componentes de GPU antes de cargar


MIN_DRIVER_CUDA = 12000  # los componentes de GPU son de CUDA 12


def driver_cuda_version(loader: Callable[[str], object] = ctypes.WinDLL) -> int | None:
    """Versión de CUDA que admite el driver NVIDIA (p. ej. 12080), o None si no hay driver."""
    if os.environ.get("CALLTX_FORCE_CPU") == "1":
        return None
    try:
        dll = loader("nvcuda.dll")
        v = ctypes.c_int(0)
        if dll.cuDriverGetVersion(ctypes.byref(v)) != 0:
            return None
        return v.value or None
    except (OSError, AttributeError):
        return None


def nvidia_status(get_version: Callable[[], int | None] = driver_cuda_version) -> str:
    """"none": sin NVIDIA; "old": driver demasiado antiguo para CUDA 12; "ok": se puede usar."""
    v = get_version()
    if v is None:
        return "none"
    return "ok" if v >= MIN_DRIVER_CUDA else "old"


def has_nvidia_driver(loader: Callable[[str], object] = ctypes.WinDLL) -> bool:
    """True si hay driver NVIDIA válido para CUDA 12."""
    return nvidia_status(lambda: driver_cuda_version(loader)) == "ok"


def resolve(settings, nvidia: bool, cuda_ready: bool) -> Resolved:
    want_gpu = settings.device in ("auto", "cuda") and nvidia
    if want_gpu:
        model = GPU_MODEL if settings.model == "auto" else settings.model
        return Resolved("cuda", "float16", model, needs_cuda=not cuda_ready)
    model = CPU_MODEL if settings.model == "auto" else settings.model
    return Resolved("cpu", "int8", model, needs_cuda=False)


def cpu_fallback(settings) -> Resolved:
    """Lo que se usa si la GPU no se pudo preparar."""
    model = CPU_MODEL if settings.model == "auto" else settings.model
    return Resolved("cpu", "int8", model, needs_cuda=False)

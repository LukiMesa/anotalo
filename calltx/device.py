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


def has_nvidia_driver(loader: Callable[[str], object] = ctypes.WinDLL) -> bool:
    """True si el PC tiene driver NVIDIA (nvcuda.dll se puede cargar)."""
    if os.environ.get("CALLTX_FORCE_CPU") == "1":
        return False
    try:
        loader("nvcuda.dll")
        return True
    except OSError:
        return False


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

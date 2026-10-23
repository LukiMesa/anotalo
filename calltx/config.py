"""Ajustes del usuario, guardados en %APPDATA%\\CallTranscriber\\config.json."""

from __future__ import annotations

import json
import logging
import os
from dataclasses import asdict, dataclass, field, fields
from pathlib import Path

log = logging.getLogger(__name__)


def _default_output_dir() -> str:
    from .paths import default_output_dir

    return str(default_output_dir())


@dataclass
class Settings:
    model: str = "auto"  # auto = large-v3-turbo con GPU, small con procesador
    language: str = "es"
    device: str = "auto"  # auto | cuda | cpu
    compute_type: str = "float16"
    label_me: str = "Yo"
    label_others: str = "Otros"
    hotkey: str = "<ctrl>+<alt>+r"
    auto_mode: bool = True
    output_dir: str = field(default_factory=_default_output_dir)
    mic_device: str | None = None
    silence_ms: int = 600
    max_segment_s: float = 25.0
    min_segment_ms: int = 300
    save_audio: bool = False
    copy_on_stop: bool = True
    show_indicator: bool = True
    autostart: bool = True
    autostart_configured: bool = False
    detect_teams: bool = True
    detect_meet: bool = True


def default_config_path() -> Path:
    from .paths import data_dir

    return data_dir() / "config.json"


def load_settings(path: Path | None = None) -> Settings:
    path = path or default_config_path()
    if not path.exists():
        return Settings()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            raise ValueError("config root must be an object")
    except (OSError, ValueError) as e:
        log.warning("Config inválida en %s (%s); uso valores por defecto", path, e)
        return Settings()
    if data.get("device") == "cuda":  # versiones anteriores: ahora se decide solo
        data["device"] = "auto"
    known = {f.name for f in fields(Settings)}
    return Settings(**{k: v for k, v in data.items() if k in known})


def save_settings(s: Settings, path: Path | None = None) -> None:
    path = path or default_config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(asdict(s), indent=2, ensure_ascii=False), encoding="utf-8")

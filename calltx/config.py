"""Ajustes del usuario, guardados en %APPDATA%\\CallTranscriber\\config.json."""

from __future__ import annotations

import json
import logging
import os
from dataclasses import asdict, dataclass, field, fields
from pathlib import Path

log = logging.getLogger(__name__)


def _default_output_dir() -> str:
    return str(Path.home() / "Documents" / "Llamadas")


@dataclass
class Settings:
    model: str = "large-v3-turbo"
    language: str = "es"
    device: str = "cuda"
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
    base = os.environ.get("APPDATA") or str(Path.home() / "AppData" / "Roaming")
    return Path(base) / "CallTranscriber" / "config.json"


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
    known = {f.name for f in fields(Settings)}
    return Settings(**{k: v for k, v in data.items() if k in known})


def save_settings(s: Settings, path: Path | None = None) -> None:
    path = path or default_config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(asdict(s), indent=2, ensure_ascii=False), encoding="utf-8")

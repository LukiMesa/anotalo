"""Fixtures de config para los tests de Anotalo (crates/core/tests/config.rs).

- config_python.json: lo que guarda la app de Python 0.6.x (su save_settings), con valores
  distintos de los de por defecto, para comprobar que Anotalo lo lee entero.
- config_anotalo.json: lo que debe guardar Anotalo, byte a byte: el mismo formato que Python
  (json.dumps con indent=2 y ensure_ascii=False, sin salto final), con las claves de Python en su
  orden y después las nuevas de Anotalo.

Uso: .venv\\Scripts\\python -I scripts\\fixtures\\config_reference.py
"""

from __future__ import annotations

import json
import sys
from dataclasses import asdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from calltx.config import Settings, save_settings  # noqa: E402

OUT = ROOT / "crates" / "core" / "tests" / "fixtures"
OUT.mkdir(parents=True, exist_ok=True)

python = Settings(
    auto_mode=False,
    hotkey="<ctrl>+<alt>+t",
    output_dir="C:\\Users\\José\\Documents\\Llamadas",
    mic_device="Micrófono (Elgato Wave:3)",
    detect_discord=True,
    release_after_min=30,
    compute_type="int8",
    update_notified="0.6.4",
)
save_settings(python, OUT / "config_python.json")

anotalo = asdict(Settings(hotkey="<ctrl>+<alt>+t", detect_discord=True,
                          output_dir="C:\\Users\\José\\Documents\\Llamadas"))
anotalo.update({"hotkey_mark": "<ctrl>+<alt>+m", "igpu_faster": None, "igpu_name": "",
                "auto_update_check": True,
                "claude_instruction": "Saca los acuerdos, tareas y pendientes de esta llamada y propón cómo ejecutarlos"})
(OUT / "config_anotalo.json").write_text(json.dumps(anotalo, indent=2, ensure_ascii=False), encoding="utf-8", newline="\n")
print("ok")

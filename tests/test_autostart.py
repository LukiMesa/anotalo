from pathlib import Path

from calltx import autostart
from calltx.config import Settings


def test_autostart_enable_disable(tmp_path):
    target = Path(r"C:\Windows\System32\notepad.exe")
    assert not autostart.is_enabled(tmp_path)
    autostart.enable(target, "--minimized", tmp_path, startup_dir=tmp_path)
    assert autostart.shortcut_path(tmp_path).exists()
    assert autostart.is_enabled(tmp_path)
    autostart.disable(tmp_path)
    assert not autostart.is_enabled(tmp_path)


def test_ensure_autostart_first_run_only(tmp_path):
    s = Settings()
    target = Path(r"C:\Windows\System32\notepad.exe")
    assert autostart.ensure_autostart(s, target, "--minimized", tmp_path, startup_dir=tmp_path) is True
    assert s.autostart_configured is True and autostart.is_enabled(tmp_path)
    autostart.disable(tmp_path)
    assert autostart.ensure_autostart(s, target, "--minimized", tmp_path, startup_dir=tmp_path) is False
    assert not autostart.is_enabled(tmp_path)

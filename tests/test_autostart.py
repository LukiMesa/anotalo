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


def test_ensure_autostart_only_first_run(tmp_path):
    s = Settings()
    target = Path(r"C:\Windows\System32\notepad.exe")
    autostart.enable(target, "--minimized", tmp_path, startup_dir=tmp_path)
    assert autostart.ensure_autostart(s, target, "--minimized", tmp_path, startup_dir=tmp_path, frozen=True) is True
    autostart.disable(tmp_path)
    assert autostart.ensure_autostart(s, target, "--minimized", tmp_path, startup_dir=tmp_path, frozen=True) is False
    assert s.autostart is True  # ya configurado: no se vuelve a leer


def test_dev_never_touches_shortcut(tmp_path):
    s = Settings()
    target = Path(r"C:\Windows\System32\notepad.exe")
    assert autostart.ensure_autostart(s, target, "--minimized", tmp_path, startup_dir=tmp_path, frozen=False) is False
    assert not autostart.is_enabled(tmp_path)


def test_frozen_first_run_reads_existing_shortcut(tmp_path):
    target = Path(r"C:\Windows\System32\notepad.exe")
    autostart.enable(target, "--minimized", tmp_path, startup_dir=tmp_path)  # lo creó el instalador
    s = Settings(autostart=False)
    assert autostart.ensure_autostart(s, target, "--minimized", tmp_path, startup_dir=tmp_path, frozen=True) is True
    assert s.autostart is True and s.autostart_configured is True


def test_frozen_first_run_without_shortcut_sets_false(tmp_path):
    target = Path(r"C:\Windows\System32\notepad.exe")
    s = Settings()
    assert autostart.ensure_autostart(s, target, "--minimized", tmp_path, startup_dir=tmp_path, frozen=True) is True
    assert s.autostart is False and not autostart.is_enabled(tmp_path)


def test_target_for_current_run_dev_is_none():
    assert autostart.target_for_current_run(frozen=False) is None

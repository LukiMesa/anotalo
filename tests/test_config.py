import json

from calltx.config import Settings, load_settings, save_settings


def test_defaults(tmp_path):
    s = load_settings(tmp_path / "missing.json")
    assert s.label_me == "Yo"
    assert s.label_others == "Otros"
    assert s.model == "auto"
    assert s.language == "es"


def test_roundtrip(tmp_path):
    p = tmp_path / "c.json"
    s = Settings(auto_mode=False, hotkey="<ctrl>+<alt>+t")
    save_settings(s, p)
    assert load_settings(p) == s


def test_unknown_keys_ignored(tmp_path):
    p = tmp_path / "c.json"
    p.write_text(json.dumps({"bogus": 1, "label_me": "Ana"}), encoding="utf-8")
    assert load_settings(p).label_me == "Ana"


def test_partial_file_keeps_defaults(tmp_path):
    p = tmp_path / "c.json"
    p.write_text(json.dumps({"auto_mode": False}), encoding="utf-8")
    s = load_settings(p)
    assert s.auto_mode is False
    assert s.model == "auto"


def test_corrupt_file_gives_defaults(tmp_path):
    p = tmp_path / "c.json"
    p.write_text("{not json", encoding="utf-8")
    assert load_settings(p) == Settings()


def test_new_defaults():
    s = Settings()
    assert s.copy_on_stop is True and s.show_indicator is True
    assert s.autostart is True and s.autostart_configured is False
    assert s.detect_teams is True and s.detect_browsers is True
    assert s.detect_zoom and s.detect_whatsapp and s.detect_slack and s.detect_discord is False


def test_defaults_auto():
    s = Settings()
    assert s.device == "auto" and s.model == "auto"


def test_migrate_cuda_to_auto(tmp_path):
    p = tmp_path / "c.json"
    p.write_text(json.dumps({"device": "cuda", "model": "large-v3-turbo", "compute_type": "float16"}), encoding="utf-8")
    s = load_settings(p)
    assert s.device == "auto" and s.model == "large-v3-turbo"


def test_release_after_default_is_ten_minutes():
    assert Settings().release_after_min == 10


def test_migrates_detect_meet_to_browsers(tmp_path):
    import json

    p = tmp_path / "config.json"
    p.write_text(json.dumps({"detect_meet": False, "detect_teams": True}), encoding="utf-8")
    s = load_settings(p)
    assert s.detect_browsers is False and s.detect_teams is True and s.detect_discord is False


def test_loads_config_saved_by_anotalo():
    """Un config.json que guardó Anotalo (Rust) se sigue leyendo en la app de Python."""
    from pathlib import Path

    fixture = Path(__file__).resolve().parents[1] / "crates" / "core" / "tests" / "fixtures" / "config_anotalo.json"
    s = load_settings(fixture)
    assert s.hotkey == "<ctrl>+<alt>+t"
    assert s.output_dir == "C:\\Users\\José\\Documents\\Llamadas"
    assert s.detect_discord is True

import json

from calltx.config import Settings, load_settings, save_settings


def test_defaults(tmp_path):
    s = load_settings(tmp_path / "missing.json")
    assert s.label_me == "Yo"
    assert s.label_others == "Otros"
    assert s.model == "large-v3-turbo"
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
    assert s.model == "large-v3-turbo"


def test_corrupt_file_gives_defaults(tmp_path):
    p = tmp_path / "c.json"
    p.write_text("{not json", encoding="utf-8")
    assert load_settings(p) == Settings()

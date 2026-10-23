import pytest

from calltx.hotkey import MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, parse_hotkey


def test_parse_default():
    assert parse_hotkey("<ctrl>+<alt>+r") == (MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, ord("R"))


def test_parse_shift_digit():
    assert parse_hotkey("<ctrl>+<shift>+1") == (MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, ord("1"))


def test_parse_function_key():
    assert parse_hotkey("<ctrl>+<f9>") == (MOD_CONTROL | MOD_NOREPEAT, 0x78)


def test_parse_invalid():
    with pytest.raises(ValueError):
        parse_hotkey("<ctrl>+<alt>")

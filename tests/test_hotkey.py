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


def test_validate_requires_modifier():
    from calltx.hotkey import validate_hotkey

    for bad in ["r", "<shift>+r"]:
        with pytest.raises(ValueError):
            validate_hotkey(bad)
    assert validate_hotkey("<ctrl>+<alt>+r")[1] == ord("R")


def _hold_hotkey(spec, ready, release):
    import ctypes

    from calltx.hotkey import parse_hotkey

    mods, vk = parse_hotkey(spec)
    ok = ctypes.windll.user32.RegisterHotKey(None, 7, mods, vk)
    ready.append(ok)
    release.wait(5)
    ctypes.windll.user32.UnregisterHotKey(None, 7)


def test_rebind_conflict_keeps_old():
    import threading

    from calltx.hotkey import GlobalHotkey, HotkeyError

    ready, release = [], threading.Event()
    t = threading.Thread(target=_hold_hotkey, args=("<ctrl>+<alt>+<f12>", ready, release))
    t.start()
    while not ready:
        pass
    hk = GlobalHotkey("<ctrl>+<alt>+<f11>", lambda: None)
    hk.start()
    try:
        with pytest.raises(HotkeyError):
            hk.rebind("<ctrl>+<alt>+<f12>")
        assert hk.spec == "<ctrl>+<alt>+<f11>"
    finally:
        hk.stop()
        release.set()
        t.join()


def test_rebind_success():
    from calltx.hotkey import GlobalHotkey

    hk = GlobalHotkey("<ctrl>+<alt>+<f10>", lambda: None)
    hk.start()
    try:
        hk.rebind("<ctrl>+<alt>+<f9>")
        assert hk.spec == "<ctrl>+<alt>+<f9>"
    finally:
        hk.stop()


def test_pause_releases_and_resume_restores():
    import ctypes
    import threading

    from calltx.hotkey import GlobalHotkey, parse_hotkey

    spec = "<ctrl>+<alt>+<f8>"
    hk = GlobalHotkey(spec, lambda: None)
    hk.start()
    mods, vk = parse_hotkey(spec)

    def try_register(out):
        ok = ctypes.windll.user32.RegisterHotKey(None, 9, mods, vk)
        out.append(bool(ok))
        if ok:
            ctypes.windll.user32.UnregisterHotKey(None, 9)

    try:
        res = []
        t = threading.Thread(target=try_register, args=(res,)); t.start(); t.join()
        assert res == [False]  # lo tiene la app
        hk.pause()
        t = threading.Thread(target=try_register, args=(res,)); t.start(); t.join()
        assert res[-1] is True  # liberado durante la captura
        hk.resume()
        t = threading.Thread(target=try_register, args=(res,)); t.start(); t.join()
        assert res[-1] is False  # vuelve a estar registrado
    finally:
        hk.stop()

from calltx.ui.indicator import render_pill


def test_render_sizes():
    img = render_pill("recording", [0.2] * 6, 42)
    assert img.mode == "RGBA"
    assert img.width < 160 and 30 <= img.height <= 36
    big = render_pill("recording", [0.2] * 6, 42, scale=1.5)
    assert 45 <= big.height <= 54


def test_render_has_transparent_corners():
    img = render_pill("recording", [0.5] * 6, 3)
    assert img.getpixel((0, 0))[3] == 0
    assert img.getpixel((img.width // 2, img.height // 2))[3] > 200


def test_render_copied_wider_than_recording():
    assert render_pill("copied", [], 0).width > render_pill("recording", [0.1] * 6, 0).width


def test_levels_change_image():
    a = render_pill("recording", [0.0] * 6, 10).tobytes()
    b = render_pill("recording", [0.9] * 6, 10).tobytes()
    assert a != b


def test_transcribing_and_saved_render():
    for state in ("transcribing", "saved"):
        img = render_pill(state, [], 0)
        assert img.height < 40


def test_win32_calls_declare_64bit_safe_types():
    # Sin argtypes/restype, ctypes trunca los handles de 64 bits (fallo aleatorio al dibujar).
    from calltx.ui import indicator as m

    for fn in [m._user32.GetDC, m._user32.ReleaseDC, m._gdi32.CreateCompatibleDC, m._gdi32.CreateDIBSection,
               m._gdi32.SelectObject, m._gdi32.DeleteObject, m._gdi32.DeleteDC, m._user32.UpdateLayeredWindow,
               m._user32.ShowWindow, m._user32.SetWindowPos, m._user32.CreateWindowExW, m._user32.DestroyWindow,
               m._user32.RegisterClassW, m._kernel32.GetModuleHandleW]:
        assert fn.argtypes is not None, fn.__name__

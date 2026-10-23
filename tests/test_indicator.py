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

from calltx.clipboard import get_text, set_text


def test_clipboard_roundtrip():
    old = get_text()
    try:
        set_text("Necesito un botón verde — ñ ✓")
        assert get_text() == "Necesito un botón verde — ñ ✓"
    finally:
        set_text(old)

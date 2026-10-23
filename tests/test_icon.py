import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load_make_icon():
    spec = importlib.util.spec_from_file_location("make_icon", ROOT / "scripts" / "make_icon.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def test_icon_render_and_file():
    m = load_make_icon()
    img = m.render(256)
    assert img.size == (256, 256) and img.mode == "RGBA"
    assert img.getpixel((0, 0))[3] == 0  # esquinas transparentes
    ico = ROOT / "calltx" / "ui" / "assets" / "icon.ico"
    assert ico.is_file()
    from PIL import Image

    sizes = Image.open(ico).info.get("sizes")
    assert {(16, 16), (32, 32), (48, 48), (256, 256)} <= set(sizes)

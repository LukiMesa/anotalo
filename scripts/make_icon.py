"""Genera el icono de la app (colores del tema): calltx/ui/assets/icon.ico y icon.png.

Uso: python scripts/make_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

NAVY = (22, 28, 43, 255)  # --surface / noche
SOFT = (19, 52, 71, 255)  # --orbita-soft
ORBITA = (46, 197, 230, 255)  # --orbita
SIZES = [16, 24, 32, 48, 64, 128, 256]
OUT = Path(__file__).resolve().parents[1] / "calltx" / "ui" / "assets"


def render(size: int) -> Image.Image:
    k = 4  # supermuestreo
    s = size * k
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((0, 0, s - 1, s - 1), radius=int(s * 0.22), fill=NAVY)
    m = int(s * 0.14)
    d.rounded_rectangle((m, m, s - 1 - m, s - 1 - m), radius=int(s * 0.16), fill=SOFT)
    w = max(2, int(s * 0.07))
    cx = s / 2
    # micrófono
    d.rounded_rectangle((cx - s * 0.11, s * 0.24, cx + s * 0.11, s * 0.56), radius=s * 0.11, fill=ORBITA)
    d.arc((cx - s * 0.2, s * 0.34, cx + s * 0.2, s * 0.66), 0, 180, fill=ORBITA, width=w)
    d.line((cx, s * 0.66, cx, s * 0.76), fill=ORBITA, width=w)
    d.line((cx - s * 0.1, s * 0.76, cx + s * 0.1, s * 0.76), fill=ORBITA, width=w)
    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    big = render(256)
    big.save(OUT / "icon.png")
    big.save(OUT / "icon.ico", sizes=[(n, n) for n in SIZES])
    print("icono creado en", OUT)


if __name__ == "__main__":
    main()

# -*- coding: utf-8 -*-
"""Generate layered pet test assets (body / eye-open / eye-closed / accent) as
transparent 512x512 PNGs into public/test-assets/layered."""
import os
import sys

from PIL import Image, ImageDraw

OUT = os.path.join("public", "test-assets", "layered")
SIZE = 512

FUR = (72, 94, 86, 255)
INNER = (220, 174, 169, 255)
EYE = (245, 205, 74, 255)
PUPIL = (40, 40, 40, 255)
BLUSH = (255, 120, 120, 160)
LINE = (60, 60, 60, 255)


def new_layer() -> Image.Image:
    return Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))


def main() -> None:
    os.makedirs(OUT, exist_ok=True)

    body = new_layer()
    d = ImageDraw.Draw(body)
    d.ellipse((106, 154, 406, 478), fill=FUR)
    d.ellipse((126, 68, 386, 304), fill=FUR)
    d.polygon([(145, 118), (168, 28), (222, 94)], fill=FUR)
    d.polygon([(290, 94), (344, 28), (367, 118)], fill=FUR)
    d.ellipse((176, 159, 231, 203), fill=INNER)
    d.ellipse((281, 159, 336, 203), fill=INNER)
    body.save(os.path.join(OUT, "body.png"))

    eye_open = new_layer()
    d = ImageDraw.Draw(eye_open)
    d.ellipse((190, 171, 212, 189), fill=EYE)
    d.ellipse((300, 171, 322, 189), fill=EYE)
    d.ellipse((196, 175, 206, 185), fill=PUPIL)
    d.ellipse((306, 175, 316, 185), fill=PUPIL)
    eye_open.save(os.path.join(OUT, "eye-open.png"))

    eye_closed = new_layer()
    d = ImageDraw.Draw(eye_closed)
    d.arc((186, 172, 216, 188), 20, 140, fill=LINE, width=4)
    d.arc((296, 172, 326, 188), 20, 140, fill=LINE, width=4)
    eye_closed.save(os.path.join(OUT, "eye-closed.png"))

    accent = new_layer()
    d = ImageDraw.Draw(accent)
    d.ellipse((150, 220, 190, 244), fill=BLUSH)
    d.ellipse((322, 220, 362, 244), fill=BLUSH)
    accent.save(os.path.join(OUT, "accent.png"))

    print(f"layered assets written to {os.path.abspath(OUT)}")


if __name__ == "__main__":
    main()

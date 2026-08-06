# -*- coding: utf-8 -*-
"""Generate placeholder built-in pets for the adoption flow (dev assets)."""
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "apps" / "desktop" / "public" / "builtin-pets"
OUT.mkdir(parents=True, exist_ok=True)


def new_canvas():
    img = Image.new("RGBA", (512, 512), (0, 0, 0, 0))
    return img, ImageDraw.Draw(img)


def make_cat():
    img, d = new_canvas()
    d.ellipse((150, 250, 362, 470), fill=(255, 175, 95, 255))
    d.ellipse((160, 105, 352, 297), fill=(255, 185, 105, 255))
    d.polygon([(185, 135), (210, 55), (250, 125)], fill=(255, 160, 80, 255))
    d.polygon([(262, 125), (302, 55), (327, 135)], fill=(255, 160, 80, 255))
    d.polygon([(200, 120), (214, 75), (240, 118)], fill=(255, 130, 90, 255))
    d.polygon([(272, 118), (298, 75), (312, 120)], fill=(255, 130, 90, 255))
    d.ellipse((205, 180, 235, 210), fill=(45, 35, 30, 255))
    d.ellipse((277, 180, 307, 210), fill=(45, 35, 30, 255))
    d.polygon([(246, 220), (266, 220), (256, 236)], fill=(255, 120, 120, 255))
    d.line((256, 236, 256, 248), fill=(120, 80, 70, 255), width=3)
    d.arc((236, 230, 276, 260), start=20, end=160, fill=(120, 80, 70, 255), width=3)
    for x in (170, 190):
        d.line((x, 215, x - 55, 205), fill=(160, 120, 100, 255), width=3)
        d.line((x, 225, x - 55, 225), fill=(160, 120, 100, 255), width=3)
    for x in (342, 322):
        d.line((x, 215, x + 55, 205), fill=(160, 120, 100, 255), width=3)
        d.line((x, 225, x + 55, 225), fill=(160, 120, 100, 255), width=3)
    d.ellipse((120, 330, 190, 470), fill=(255, 175, 95, 255))
    d.ellipse((160, 440, 220, 475), fill=(255, 205, 150, 255))
    d.ellipse((292, 440, 352, 475), fill=(255, 205, 150, 255))
    return img


def make_dog():
    img, d = new_canvas()
    d.ellipse((150, 280, 362, 470), fill=(215, 170, 115, 255))
    d.ellipse((170, 125, 342, 295), fill=(225, 180, 125, 255))
    d.ellipse((120, 140, 205, 245), fill=(190, 130, 80, 255))
    d.ellipse((307, 140, 392, 245), fill=(190, 130, 80, 255))
    d.ellipse((215, 215, 297, 280), fill=(250, 240, 225, 255))
    d.ellipse((242, 222, 270, 250), fill=(35, 30, 28, 255))
    d.ellipse((205, 190, 240, 225), fill=(35, 30, 28, 255))
    d.ellipse((272, 190, 307, 225), fill=(35, 30, 28, 255))
    d.ellipse((252, 262, 278, 290), fill=(245, 150, 150, 255))
    d.ellipse((120, 330, 200, 470), fill=(215, 170, 115, 255))
    d.ellipse((160, 440, 220, 475), fill=(235, 210, 180, 255))
    d.ellipse((292, 440, 352, 475), fill=(235, 210, 180, 255))
    return img


make_cat().save(OUT / "cat-1.png")
make_dog().save(OUT / "dog-1.png")
print(f"written to {OUT}")

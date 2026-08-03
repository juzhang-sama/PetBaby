# -*- coding: utf-8 -*-
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from PIL import Image, ImageDraw  # noqa: E402

from postprocess import (  # noqa: E402
    BackgroundRemovalError,
    estimate_background_color,
    is_uniform_background,
    remove_background,
    remove_background_chroma,
)


def make_test_image(bg: tuple[int, int, int], size=(200, 200), with_subject=False) -> Image.Image:
    img = Image.new("RGB", size, bg)
    if with_subject:
        draw = ImageDraw.Draw(img)
        draw.ellipse((60, 60, 140, 140), fill=(72, 94, 86))
    return img


def test_estimate_background_color():
    img = make_test_image((226, 226, 226))
    assert estimate_background_color(img) == (226, 226, 226)


def test_uniform_background_detection():
    assert is_uniform_background(make_test_image((226, 226, 226)))
    # noisy border should not be uniform
    img = make_test_image((226, 226, 226), with_subject=True)
    assert is_uniform_background(img)


def test_chroma_removal_makes_corners_transparent():
    img = make_test_image((226, 226, 226), with_subject=True)
    out = remove_background_chroma(img)
    assert out.mode == "RGBA"
    assert out.getpixel((5, 5))[3] < 40  # corner transparent
    assert out.getpixel((100, 100))[3] > 200  # subject opaque


def test_auto_uses_chroma_for_uniform_background():
    img = make_test_image((226, 226, 226), with_subject=True)
    out = remove_background(img, method="auto")
    assert out.getpixel((5, 5))[3] < 40


def test_unknown_method_raises():
    img = make_test_image((226, 226, 226))
    try:
        remove_background(img, method="nope")
        assert False, "expected error"
    except BackgroundRemovalError:
        pass

# -*- coding: utf-8 -*-
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from PIL import Image, ImageDraw  # noqa: E402

from filter import filter_candidates  # noqa: E402


def make_rgba(size=(512, 512), opaque=False) -> Image.Image:
    img = Image.new("RGBA", size, (0, 0, 0, 0))
    if opaque:
        draw = ImageDraw.Draw(img)
        draw.ellipse((50, 50, 460, 460), fill=(72, 94, 86, 255))
    return img


def test_all_kept_when_clean():
    report = filter_candidates([make_rgba(opaque=True), make_rgba(opaque=True)])
    assert report.kept == 2
    assert report.rejected == []


def test_missing_candidate_rejected():
    report = filter_candidates([None, make_rgba(opaque=True)])
    assert report.kept == 1
    assert report.rejected[0][1] == "missing"


def test_non_rgba_rejected():
    rgb = Image.new("RGB", (512, 512), (226, 226, 226))
    report = filter_candidates([rgb])
    assert report.rejected[0][1] == "not-transparent"


def test_too_small_rejected():
    small = make_rgba(size=(100, 100), opaque=True)
    report = filter_candidates([small])
    assert report.rejected[0][1].startswith("too-small")


def test_blank_content_rejected():
    report = filter_candidates([make_rgba(opaque=False)])
    assert report.rejected[0][1] == "blank-content"

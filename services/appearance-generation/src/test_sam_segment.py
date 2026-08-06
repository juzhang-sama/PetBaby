# -*- coding: utf-8 -*-
"""Tests for the MobileSAM ONNX refinement step."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import numpy as np  # noqa: E402
import pytest  # noqa: E402
from PIL import Image  # noqa: E402

from sam_segment import (  # noqa: E402
    MobileSam,
    preprocess_for_encoder,
    scale_boxes_to_model,
)


def test_preprocess_for_encoder_normalizes_and_scales():
    image = Image.new("RGB", (800, 400), (128, 128, 128))
    arr, scale = preprocess_for_encoder(image)
    assert arr.shape == (1024, 1024, 3)
    assert arr.dtype == np.float32
    assert scale == 1024 / 800
    # center pixel of the resized content: (128-123.675)/58.395
    expected = (128.0 - 123.675) / 58.395
    assert arr[200, 400, 0] == pytest.approx(expected, abs=1e-4)
    # padded area is black -> normalized mean value
    assert arr[900, 900, 0] == pytest.approx(-123.675 / 58.395, abs=1e-4)


def test_scale_boxes_to_model():
    boxes = {"head": (100, 50, 200, 150), "tail": (10, 20, 30, 40)}
    scaled = scale_boxes_to_model(boxes, scale=2.0)
    assert scaled["head"] == (200, 100, 400, 300)
    assert scaled["tail"] == (20, 40, 60, 80)


def test_mobile_sam_unavailable_without_models(tmp_path):
    sam = MobileSam(model_dir=tmp_path)
    assert sam.available() is False


class FakeSam:
    """Stand-in MobileSam used by layering integration tests."""

    def __init__(self, masks):
        self._masks = masks

    def segment_boxes(self, image, boxes):
        return self._masks


def test_build_layer_set_uses_sam_refined_masks(tmp_path, monkeypatch):
    from layering import build_layer_set, parse_part_layers

    SIZE = 200

    def norm(points):
        return [[x / SIZE, y / SIZE] for x, y in points]

    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    from PIL import ImageDraw

    draw = ImageDraw.Draw(img)
    draw.rectangle((60, 120, 140, 180), fill=(180, 60, 60, 255))
    draw.rectangle((70, 50, 130, 120), fill=(60, 180, 60, 255))
    draw.rectangle((80, 70, 96, 86), fill=(40, 40, 220, 255))
    draw.rectangle((104, 70, 120, 86), fill=(40, 40, 220, 255))
    raw = {
        "parts": [
            {
                "role": "body",
                "polygon": norm([(60, 120), (140, 120), (140, 180), (60, 180)]),
                "pivot": norm([(100, 180)])[0],
            },
            {
                "role": "head",
                "polygon": norm([(70, 50), (130, 50), (130, 120), (70, 120)]),
                "pivot": norm([(100, 120)])[0],
            },
            {
                "role": "leftEye",
                "polygon": norm([(80, 70), (96, 70), (96, 86), (80, 86)]),
                "pivot": norm([(88, 78)])[0],
            },
            {
                "role": "rightEye",
                "polygon": norm([(104, 70), (120, 70), (120, 86), (104, 86)]),
                "pivot": norm([(112, 78)])[0],
            },
        ],
        "zOrder": ["body", "head", "leftEye", "rightEye"],
    }
    parts, z_order = parse_part_layers(raw)
    alpha = np.array(img)[..., 3] > 0
    # Fake SAM claims the whole head as the left eye: refinement must win.
    fake_masks = {
        "body": np.array(img)[..., 3] > 0,
        "head": np.array(img)[..., 3] > 0,
        "leftEye": np.zeros((SIZE, SIZE), dtype=bool),
        "rightEye": np.zeros((SIZE, SIZE), dtype=bool),
    }
    fake_masks["leftEye"][70:120, 70:130] = True
    sam = FakeSam(fake_masks)
    layer_set = build_layer_set(img, parts, z_order, dilate_px=0, pad=2, sam=sam)
    assert layer_set.coverage == pytest.approx(1.0)
    assert layer_set.layers["leftEye"].image.size[0] >= 60

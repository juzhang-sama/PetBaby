# -*- coding: utf-8 -*-
"""Tests for the single-image part-layer decomposition pipeline."""
import json
import sys
from types import SimpleNamespace
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import numpy as np  # noqa: E402
import pytest  # noqa: E402
from PIL import Image, ImageDraw  # noqa: E402

from layering import (  # noqa: E402
    DEFAULT_Z_ORDER,
    ExtractedLayer,
    PartLayerAnalyzer,
    PART_COLORS,
    assign_layer_pixels,
    build_layer_set,
    compose_layers,
    derive_pivots,
    extract_layers,
    eye_content_ok,
    fix_left_right_perspective,
    parse_part_layers,
    quality_score,
    rasterize_masks,
    sam_prompt_boxes,
    save_layer_set,
)


SIZE = 200


def norm(points):
    return [[x / SIZE, y / SIZE] for x, y in points]


def make_synthetic_image():
    """Body + head + two eyes on a transparent canvas (200x200)."""
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    draw.rectangle((60, 120, 140, 180), fill=(180, 60, 60, 255))
    draw.rectangle((70, 50, 130, 120), fill=(60, 180, 60, 255))
    draw.rectangle((80, 70, 96, 86), fill=(40, 40, 220, 255))
    draw.rectangle((104, 70, 120, 86), fill=(40, 40, 220, 255))
    return img


def synthetic_parts_raw():
    return {
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


def test_parse_part_layers_accepts_valid_raw():
    parts, z_order = parse_part_layers(synthetic_parts_raw())
    assert set(parts) == {"body", "head", "leftEye", "rightEye"}
    assert z_order == ["body", "head", "leftEye", "rightEye"]
    body = parts["body"]
    assert body.polygon[0] == pytest.approx((0.3, 0.6))
    assert body.pivot == pytest.approx((0.5, 0.9))


def test_parse_part_layers_rejects_duplicate_role():
    raw = synthetic_parts_raw()
    raw["parts"].append(dict(raw["parts"][0]))
    with pytest.raises(ValueError):
        parse_part_layers(raw)


def test_parse_part_layers_rejects_short_polygon():
    raw = synthetic_parts_raw()
    raw["parts"][1]["polygon"] = [[0.5, 0.5], [0.6, 0.6]]
    with pytest.raises(ValueError):
        parse_part_layers(raw)


def test_parse_part_layers_clamps_out_of_range_points():
    raw = synthetic_parts_raw()
    raw["parts"][0]["polygon"] = [[1.5, -0.2], [0.7, 0.7], [0.8, 0.8]]
    parts, _ = parse_part_layers(raw)
    assert parts["body"].polygon[0] == (1.0, 0.0)


def test_parse_part_layers_default_z_order_when_missing():
    raw = synthetic_parts_raw()
    raw.pop("zOrder")
    parts, z_order = parse_part_layers(raw)
    assert z_order == ["body", "head", "leftEye", "rightEye"]
    assert all(role in DEFAULT_Z_ORDER for role in z_order)


def test_parse_part_layers_enforces_canonical_head_ear_eye_order():
    raw = synthetic_parts_raw()
    raw["parts"].append(
        {
            "role": "tail",
            "polygon": norm([(150, 160), (180, 160), (180, 190), (150, 190)]),
            "pivot": norm([(165, 190)])[0],
        }
    )
    raw["parts"].append(
        {
            "role": "leftEar",
            "polygon": norm([(70, 30), (100, 30), (85, 60)]),
            "pivot": norm([(85, 60)])[0],
        }
    )
    raw["parts"].append(
        {
            "role": "rightEar",
            "polygon": norm([(110, 30), (140, 30), (125, 60)]),
            "pivot": norm([(125, 60)])[0],
        }
    )
    raw["zOrder"] = ["head", "leftEar", "rightEar", "tail", "body", "leftEye", "rightEye"]
    parts, z_order = parse_part_layers(raw)
    # Tail is behind the body (model order), head/ears/eyes keep canonical order.
    assert z_order == ["tail", "body", "head", "leftEar", "rightEar", "leftEye", "rightEye"]


def test_rasterize_masks():
    parts, z_order = parse_part_layers(synthetic_parts_raw())
    masks = rasterize_masks(parts, (SIZE, SIZE))
    assert set(masks) == {"body", "head", "leftEye", "rightEye"}
    assert masks["body"].shape == (SIZE, SIZE)
    assert masks["body"].dtype == bool
    assert masks["body"][130, 100]
    assert not masks["body"][130, 40]


def test_assign_layer_pixels_topmost_wins_and_coverage():
    parts, z_order = parse_part_layers(synthetic_parts_raw())
    masks = rasterize_masks(parts, (SIZE, SIZE))
    image = make_synthetic_image()
    alpha = np.array(image)[..., 3]
    assigned, coverage = assign_layer_pixels(masks, alpha, z_order)
    assert coverage == pytest.approx(1.0)
    assert assigned["head"][100, 100]
    assert not assigned["body"][100, 100]
    assert assigned["body"][150, 100]
    assert assigned["leftEye"][78, 88]
    assert not assigned["head"][78, 88]


def test_extract_layers_tight_crop_and_pivot():
    parts, z_order = parse_part_layers(synthetic_parts_raw())
    masks = rasterize_masks(parts, (SIZE, SIZE))
    image = make_synthetic_image()
    alpha = np.array(image)[..., 3]
    assigned, _ = assign_layer_pixels(masks, alpha, z_order)
    pivots = {role: part.pivot for role, part in parts.items()}
    layers = extract_layers(image, assigned, pivots, pad=2)
    head = next(layer for layer in layers if layer.role == "head")
    ys, xs = np.where(assigned["head"])
    expect_left = int(xs.min()) - 2
    expect_top = int(ys.min()) - 2
    expect_w = int(xs.max()) - int(xs.min()) + 1 + 4
    expect_h = int(ys.max()) - int(ys.min()) + 1 + 4
    assert head.image.size == (expect_w, expect_h)
    assert head.origin == (expect_left, expect_top)
    assert head.pivot[0] == pytest.approx((100 - expect_left) / expect_w)
    assert head.pivot[1] == pytest.approx((120 - expect_top) / expect_h)
    assert head.anchor == head.pivot


def test_build_layer_set_roundtrip():
    image = make_synthetic_image()
    parts, z_order = parse_part_layers(synthetic_parts_raw())
    layer_set = build_layer_set(image, parts, z_order, dilate_px=0, pad=2)
    assert layer_set.coverage == pytest.approx(1.0)
    assert layer_set.diff == pytest.approx(0.0)
    composed = compose_layers(layer_set.layers, image.size)
    assert np.array_equal(np.array(composed), np.array(image))
    assert [p["role"] for p in layer_set.parts] == ["body", "head", "leftEye", "rightEye"]
    assert all(p["zIndex"] == index for index, p in enumerate(layer_set.parts))
    head = next(p for p in layer_set.parts if p["role"] == "head")
    assert head["boneId"] == "head"
    assert head["deformable"] is True
    assert 0 <= head["pivot"]["x"] <= 1 and 0 <= head["pivot"]["y"] <= 1


def test_coverage_and_diff_report_missing_pixels():
    image = make_synthetic_image()
    raw = synthetic_parts_raw()
    # Drop the body: head/eye masks cannot cover the torso, so coverage < 1.
    raw["parts"] = [p for p in raw["parts"] if p["role"] != "body"]
    parts, z_order = parse_part_layers(raw)
    layer_set = build_layer_set(
        image, parts, z_order, dilate_px=0, pad=2, nearest_fallback=False
    )
    assert 0 < layer_set.coverage < 1
    assert layer_set.diff > 0


def test_nearest_fallback_assigns_uncovered_pixels():
    image = make_synthetic_image()
    raw = synthetic_parts_raw()
    # Shrink the body polygon so a strip of the torso (x 60..79) is uncovered.
    raw["parts"][0]["polygon"] = norm(
        [(80, 120), (140, 120), (140, 180), (80, 180)]
    )
    parts, z_order = parse_part_layers(raw)
    layer_set = build_layer_set(image, parts, z_order, dilate_px=0, pad=2)
    assert layer_set.coverage == pytest.approx(1.0)
    composed = compose_layers(layer_set.layers, image.size)
    pixel = np.array(composed)[150, 70]
    assert pixel[3] > 0
    assert tuple(pixel[:3]) == (180, 60, 60)


def test_analyzer_uses_chat_json_and_falls_back(monkeypatch):
    analyzer = PartLayerAnalyzer("key", "https://example.invalid", "gpt-4o")
    raw = synthetic_parts_raw()
    monkeypatch.setattr(analyzer, "_chat_json", lambda *args, **kwargs: raw)
    result = analyzer.analyze((b"fake-png", "image/png"), "cat")
    assert result == raw
    monkeypatch.setattr(analyzer, "_chat_json", lambda *args, **kwargs: None)
    assert analyzer.analyze((b"fake-png", "image/png"), "cat") is None


def test_derive_pivots_from_mask_geometry():
    image = make_synthetic_image()
    parts, z_order = parse_part_layers(synthetic_parts_raw())
    masks = rasterize_masks(parts, (SIZE, SIZE))
    alpha = np.array(image)[..., 3]
    assigned, _ = assign_layer_pixels(masks, alpha, z_order)
    pivots = derive_pivots(assigned, image.size)
    assert pivots["body"] == pytest.approx((0.5, 0.9))
    assert pivots["head"] == pytest.approx((0.5, 0.6))
    assert pivots["leftEye"][0] < 0.5 < pivots["rightEye"][0]
    assert pivots["leftEye"][1] == pytest.approx(pivots["rightEye"][1])


def test_fix_left_right_perspective_swaps_when_needed():
    size = (100, 100)
    left = np.zeros(size, dtype=bool)
    left[10:20, 60:70] = True  # physically on the right side
    right = np.zeros(size, dtype=bool)
    right[10:20, 30:40] = True  # physically on the left side
    fixed = fix_left_right_perspective(
        {"leftEar": left, "rightEar": right, "body": np.zeros(size, dtype=bool)}
    )
    assert np.array_equal(fixed["leftEar"], right)
    assert np.array_equal(fixed["rightEar"], left)


def test_build_layer_set_uses_derived_pivots():
    image = make_synthetic_image()
    raw = synthetic_parts_raw()
    # Give the model bogus pivots; geometry must win.
    raw["parts"][2]["pivot"] = [0.5, 0.5]
    raw["parts"][3]["pivot"] = [0.5, 0.5]
    parts, z_order = parse_part_layers(raw)
    layer_set = build_layer_set(image, parts, z_order, dilate_px=0, pad=2)
    by_role = {p["role"]: p for p in layer_set.parts}
    # Derived eye center is (88, 78) in full image; left-eye layer origin is
    # (78, 68) and its crop is 21x21, so the crop-relative pivot is 10/21.
    assert by_role["leftEye"]["pivot"]["x"] == pytest.approx(10 / 21)
    assert by_role["leftEye"]["pivot"]["y"] == pytest.approx(10 / 21)
    assert by_role["rightEye"]["pivot"]["x"] == pytest.approx(10 / 21)
    assert by_role["rightEye"]["pivot"]["y"] == pytest.approx(10 / 21)


def _fake_layer(role, origin, pixel):
    img = Image.new("RGBA", (10, 10), (0, 0, 0, 0))
    img.putpixel((pixel[0], pixel[1]), (255, 255, 255, 255))
    return ExtractedLayer(
        role=role,
        image=img,
        origin=origin,
        pivot=(0.5, 0.5),
        anchor=(0.5, 0.5),
    )


def test_quality_score_rewards_correct_geometry():
    good = {
        "body": _fake_layer("body", (0, 100), (5, 5)),
        "head": _fake_layer("head", (0, 0), (5, 5)),
        "leftEar": _fake_layer("leftEar", (0, 0), (2, 2)),
        "rightEar": _fake_layer("rightEar", (50, 0), (2, 2)),
        "leftEye": _fake_layer("leftEye", (0, 20), (2, 2)),
        "rightEye": _fake_layer("rightEye", (50, 20), (2, 2)),
        "tail": _fake_layer("tail", (0, 150), (5, 5)),
    }
    score = quality_score(SimpleNamespace(layers=good, parts=[]))
    assert score >= 7


def test_quality_score_penalizes_swapped_ears():
    bad = {
        "body": _fake_layer("body", (0, 100), (5, 5)),
        "head": _fake_layer("head", (0, 0), (5, 5)),
        "leftEar": _fake_layer("leftEar", (50, 0), (2, 2)),
        "rightEar": _fake_layer("rightEar", (0, 0), (2, 2)),
        "leftEye": _fake_layer("leftEye", (0, 20), (2, 2)),
        "rightEye": _fake_layer("rightEye", (50, 20), (2, 2)),
    }
    good = {
        "body": _fake_layer("body", (0, 100), (5, 5)),
        "head": _fake_layer("head", (0, 0), (5, 5)),
        "leftEar": _fake_layer("leftEar", (0, 0), (2, 2)),
        "rightEar": _fake_layer("rightEar", (50, 0), (2, 2)),
        "leftEye": _fake_layer("leftEye", (0, 20), (2, 2)),
        "rightEye": _fake_layer("rightEye", (50, 20), (2, 2)),
    }
    assert quality_score(SimpleNamespace(layers=bad, parts=[])) < quality_score(
        SimpleNamespace(layers=good, parts=[])
    )


def test_quality_score_penalizes_ears_reaching_into_head_lower_half():
    good = {
        "body": _fake_layer("body", (0, 100), (5, 5)),
        "head": _fake_layer("head", (0, 0), (5, 5)),
        "leftEar": _fake_layer("leftEar", (0, 0), (2, 2)),
        "rightEar": _fake_layer("rightEar", (50, 0), (2, 2)),
    }
    low_ear = {
        "body": _fake_layer("body", (0, 100), (5, 5)),
        "head": _fake_layer("head", (0, 0), (5, 5)),
        "leftEar": _fake_layer("leftEar", (0, 100), (2, 2)),
        "rightEar": _fake_layer("rightEar", (50, 0), (2, 2)),
    }
    assert quality_score(SimpleNamespace(layers=low_ear, parts=[])) < quality_score(
        SimpleNamespace(layers=good, parts=[])
    )


def test_sam_prompt_boxes_unions_ears_into_head():
    parts, _z_order = parse_part_layers(synthetic_parts_raw())
    boxes = sam_prompt_boxes(parts, (SIZE, SIZE))
    assert boxes["head"] == (70, 50, 130, 120)

    raw = synthetic_parts_raw()
    raw["parts"].append(
        {
            "role": "leftEar",
            "polygon": norm([(70, 20), (95, 20), (82, 55)]),
            "pivot": norm([(82, 55)])[0],
        }
    )
    parts, _z_order = parse_part_layers(raw)
    boxes = sam_prompt_boxes(parts, (SIZE, SIZE))
    # head box is expanded to include the ear above it
    assert boxes["head"] == (70, 20, 130, 120)


def test_save_layer_set_writes_files(tmp_path):
    image = make_synthetic_image()
    parts, z_order = parse_part_layers(synthetic_parts_raw())
    layer_set = build_layer_set(image, parts, z_order, dilate_px=0, pad=2)
    summary = save_layer_set(tmp_path, image, layer_set, z_order)
    for role in ("body", "head", "leftEye", "rightEye"):
        assert (tmp_path / "layers" / f"{role}.png").exists()
    assert (tmp_path / "parts.json").exists()
    assert (tmp_path / "preview.png").exists()
    assert summary["coverage"] == pytest.approx(1.0)
    manifest = json.loads((tmp_path / "parts.json").read_text(encoding="utf-8"))
    assert [p["role"] for p in manifest["parts"]] == [
        "body",
        "head",
        "leftEye",
        "rightEye",
    ]


def test_save_layer_set_writes_segmentation_map(tmp_path):
    image = make_synthetic_image()
    parts, z_order = parse_part_layers(synthetic_parts_raw())
    layer_set = build_layer_set(image, parts, z_order, dilate_px=0, pad=2)
    summary = save_layer_set(tmp_path, image, layer_set, z_order)
    segmentation = Image.open(summary["segmentation"])
    assert segmentation.size == image.size
    head_pixel = np.array(segmentation)[100, 100]
    assert tuple(head_pixel[:3]) == PART_COLORS["head"]
    body_pixel = np.array(segmentation)[150, 100]
    assert tuple(body_pixel[:3]) == PART_COLORS["body"]


def test_eye_content_ok_rejects_fur_only_eye_layers():
    from layering import ExtractedLayer

    src = Image.new("RGBA", (100, 100), (0, 0, 0, 0))
    draw = ImageDraw.Draw(src)
    draw.rectangle((0, 0, 100, 100), fill=(128, 128, 128, 255))
    draw.rectangle((10, 10, 30, 30), fill=(30, 30, 30, 255))
    eye_layer = Image.new("RGBA", (10, 10), (0, 0, 0, 0))
    eye_draw = ImageDraw.Draw(eye_layer)
    eye_draw.rectangle((0, 0, 10, 10), fill=(255, 255, 255, 255))

    def layer_at(origin):
        return ExtractedLayer(
            role="leftEye",
            image=eye_layer,
            origin=origin,
            pivot=(0.5, 0.5),
            anchor=(0.5, 0.5),
        )

    fur_set = SimpleNamespace(
        layers={"leftEye": layer_at((0, 0)), "rightEye": layer_at((0, 0))},
        parts=[],
    )
    assert not eye_content_ok(fur_set, src)
    eye_set = SimpleNamespace(
        layers={"leftEye": layer_at((10, 10)), "rightEye": layer_at((10, 10))},
        parts=[],
    )
    assert eye_content_ok(eye_set, src)

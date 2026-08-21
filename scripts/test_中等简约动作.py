from __future__ import annotations

import json
from pathlib import Path
import sys

import numpy as np
from PIL import Image, ImageDraw
import pytest
from pydantic import ValidationError

sys.path.insert(0, str(Path(__file__).resolve().parent))

from 中等简约动作 import (  # noqa: E402
    ActionAuditSpec,
    MotionAnnotation,
    audit_action,
    make_blink,
    make_breath,
    make_tail_wag,
)
from 中等简约产物 import (  # noqa: E402
    ActionExportSpec,
    export_action,
    load_logical_rgba,
    write_overview,
)


def _annotation() -> MotionAnnotation:
    return MotionAnnotation.model_validate_json(
        json.dumps(
            {
                "logicalGridSize": 160,
                "eyes": {
                    "left": [57, 48, 65, 56],
                    "right": [91, 48, 99, 56],
                },
                "breathZone": [32, 18, 127, 139],
                "groundAnchors": [[38, 140, 121, 159]],
                "tail": {
                    "enabled": True,
                    "root": [126, 129],
                    "mask": [[125, 126], [151, 114], [155, 137], [132, 143]],
                    "disabledReason": None,
                },
            }
        )
    )


def _source() -> np.ndarray:
    image = Image.new("RGBA", (160, 160), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.ellipse((32, 18, 127, 151), fill=(66, 58, 52, 255))
    draw.rectangle((38, 140, 121, 159), fill=(54, 48, 44, 255))
    draw.polygon(((125, 126), (151, 114), (155, 137), (132, 143)), fill=(66, 58, 52, 255))
    draw.rectangle((57, 48, 65, 56), fill=(94, 156, 82, 255))
    draw.rectangle((91, 48, 99, 56), fill=(94, 156, 82, 255))
    return np.asarray(image, dtype=np.uint8)


def _enclosed_transparent_pixels(frame: np.ndarray, rect: tuple[int, int, int, int]) -> int:
    x0, y0, x1, y1 = rect
    visible = frame[y0 : y1 + 1, x0 : x1 + 1, 3] > 0
    enclosed = (
        ~visible[1:-1, 1:-1]
        & visible[:-2, 1:-1]
        & visible[2:, 1:-1]
        & visible[1:-1, :-2]
        & visible[1:-1, 2:]
    )
    return int(enclosed.sum())


def test_annotation_rejects_enabled_tail_without_root() -> None:
    raw = _annotation().model_dump(by_alias=True)
    raw["tail"]["root"] = None

    with pytest.raises(ValidationError, match="root"):
        MotionAnnotation.model_validate(raw)


@pytest.mark.parametrize(
    "field,value",
    (
        ("breathZone", [32, 18, 127, 160]),
        ("breathZone", [127, 18, 32, 139]),
    ),
)
def test_annotation_rejects_invalid_rectangles(field: str, value: list[int]) -> None:
    raw = _annotation().model_dump(by_alias=True)
    raw[field] = value

    with pytest.raises(ValidationError):
        MotionAnnotation.model_validate(raw)


def test_three_required_actions_close_loop_preserve_anchors_and_palette() -> None:
    source = _source()
    annotation = _annotation()
    actions = (
        ("breath", make_breath(source, annotation), annotation.breath_zone),
        ("blink", make_blink(source, annotation), annotation.eye_bounds),
        ("tail-wag", make_tail_wag(source, annotation), annotation.tail.bounds),
    )

    for action_id, frames, target in actions:
        report = audit_action(ActionAuditSpec(action_id, annotation, target), source, frames)
        assert report.loop_closed is True
        assert report.ground_anchor_changed_pixels == 0
        assert report.maximum_visible_colors <= 24
        assert report.peak_changed_pixels > 0


def test_full_blink_ignores_one_pixel_eye_color_spill() -> None:
    source = _source().copy()
    annotation = _annotation()
    eye_color = np.array((94, 156, 82, 255), dtype=np.uint8)
    lid_color = np.array((20, 18, 16, 255), dtype=np.uint8)
    for x0, y0, x1, y1 in annotation.eyes.values():
        source[y0 - 1 : y1 + 2, x0 : x1 + 1] = eye_color
        source[(y0 + y1) // 2, (x0 + x1) // 2] = lid_color

    closed = make_blink(source, annotation)[2]

    for x0, y0, x1, y1 in annotation.eyes.values():
        eye = closed[y0 : y1 + 1, x0 : x1 + 1]
        assert not np.any(np.all(eye == eye_color, axis=2))


def test_full_blink_uses_solid_lids_instead_of_vertical_bands() -> None:
    source = _source().copy()
    annotation = _annotation()
    stripe_colors = np.array(
        ((72, 60, 48, 255), (96, 80, 64, 255), (120, 104, 88, 255)),
        dtype=np.uint8,
    )
    for x0, y0, x1, y1 in annotation.eyes.values():
        width = x1 - x0 + 1
        stripes = stripe_colors[np.arange(width) % len(stripe_colors)]
        source[y0 - 2, x0 : x1 + 1] = stripes
        source[y1 + 2, x0 : x1 + 1] = stripes[::-1]

    closed = make_blink(source, annotation)[2]

    for x0, y0, x1, y1 in annotation.eyes.values():
        colors = np.unique(
            closed[y0 : y1 + 1, x0 : x1 + 1].reshape(-1, 4), axis=0
        )
        assert len(colors) <= 3


def test_tail_wag_moves_one_tail_without_growing_visible_area() -> None:
    source = _source()
    annotation = _annotation()
    source_visible = source[:, :, 3] > 0

    for index in (1, 2, 3, 5, 6, 7):
        frame_visible = make_tail_wag(source, annotation)[index][:, :, 3] > 0
        removed = int((source_visible & ~frame_visible).sum())
        added = int((~source_visible & frame_visible).sum())
        assert removed > 0
        assert added > 0
        assert abs(added - removed) <= 64


def test_tail_wag_does_not_open_enclosed_transparent_pixels() -> None:
    source = _source()
    annotation = _annotation()
    baseline = _enclosed_transparent_pixels(source, annotation.tail.bounds)

    for frame in make_tail_wag(source, annotation):
        assert _enclosed_transparent_pixels(frame, annotation.tail.bounds) <= baseline


def test_action_export_keeps_rgba_grid_and_writes_review_gif(tmp_path: Path) -> None:
    source_path = tmp_path / "母版.png"
    Image.fromarray(_source(), "RGBA").resize((1024, 1024), Image.Resampling.NEAREST).save(
        source_path
    )
    logical = load_logical_rgba(source_path)
    frames = make_blink(logical, _annotation())

    artifact = export_action(
        ActionExportSpec("blink", tmp_path / "动作", 150),
        frames,
    )

    assert logical.shape == (160, 160, 4)
    assert artifact.gif_path.is_file()
    assert len(artifact.frame_paths) == len(frames)
    with Image.open(artifact.frame_paths[0]) as frame:
        assert frame.mode == "RGBA"
        assert frame.size == (1024, 1024)


def test_overview_supports_readable_multirow_action_grid(tmp_path: Path) -> None:
    source_path = tmp_path / "母版.png"
    Image.fromarray(_source(), "RGBA").save(source_path)

    overview = write_overview(
        tuple((f"宠物 / action-{index}", source_path) for index in range(5)),
        tmp_path / "总览.png",
        columns=4,
    )

    with Image.open(overview) as image:
        assert image.size == (2048, 384)

#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy>=2.3,<2.4", "pillow>=11.3,<11.4"]
# ///

# ─── How to run ───
# 1. Install uv (if not installed):
#      curl -LsSf https://astral.sh/uv/install.sh | sh
# 2. Run the covering tests from the repository root:
#      uv run scripts/test_中等简约动作.py
# 3. This module is imported by scripts/中等简约动作.py.
# ──────────────────

from __future__ import annotations

from collections import deque
from dataclasses import dataclass
from typing import Final

import numpy as np
from PIL import Image, ImageDraw

type Point = tuple[int, int]

LOGICAL_GRID_SIZE: Final = 160
ROOT_SEAM_LENGTH: Final = 7
TAIL_WAG_ANGLES: Final = (0, 2, 4, 2, 0, -2, -4, -2, 0)


@dataclass(frozen=True, slots=True)
class TailLayers:
    """Static repaired baseplate, movable tail pixels, and fixed root seam."""

    baseplate: np.ndarray
    tail_pixels: np.ndarray
    owned: np.ndarray
    seam: np.ndarray
    root: Point
    source: np.ndarray


def make_tail_wag_frames(
    source: np.ndarray, mask_points: tuple[Point, ...], root: Point
) -> tuple[np.ndarray, ...]:
    """Build loop-closing tail-wag frames from separately composited layers."""
    layers = _build_tail_layers(source, mask_points, root)
    return tuple(_compose_tail_frame(layers, angle) for angle in TAIL_WAG_ANGLES)


def _build_tail_layers(
    source: np.ndarray, mask_points: tuple[Point, ...], root: Point
) -> TailLayers:
    polygon = Image.new("L", (LOGICAL_GRID_SIZE, LOGICAL_GRID_SIZE), 0)
    ImageDraw.Draw(polygon).polygon(mask_points, fill=255)
    region = np.asarray(polygon, dtype=np.uint8) > 0
    owned = region & (source[:, :, 3] > 0)
    seam = owned & _root_seam_mask(owned.shape, root)
    tail_pixels = np.zeros_like(source)
    tail_pixels[owned] = source[owned]
    return TailLayers(
        _repair_baseplate(source, owned), tail_pixels, owned, seam, root, source
    )


def _root_seam_mask(shape: tuple[int, int], root: Point) -> np.ndarray:
    y_coordinates, x_coordinates = np.ogrid[: shape[0], : shape[1]]
    return (x_coordinates - root[0]) ** 2 + (y_coordinates - root[1]) ** 2 <= (
        ROOT_SEAM_LENGTH // 2
    ) ** 2


def _repair_baseplate(source: np.ndarray, owned: np.ndarray) -> np.ndarray:
    repaired = source.copy()
    available = ~owned
    queued = np.zeros_like(owned)
    queue = deque((int(y), int(x)) for y, x in zip(*np.nonzero(available), strict=True))
    queued[available] = True
    while queue:
        y, x = queue.popleft()
        for next_y, next_x in _neighbors(y, x):
            if queued[next_y, next_x]:
                continue
            repaired[next_y, next_x] = repaired[y, x]
            queued[next_y, next_x] = True
            queue.append((next_y, next_x))
    return repaired


def _neighbors(y: int, x: int) -> tuple[Point, ...]:
    return tuple(
        (next_y, next_x)
        for next_y, next_x in ((y - 1, x), (y, x - 1), (y, x + 1), (y + 1, x))
        if 0 <= next_y < LOGICAL_GRID_SIZE and 0 <= next_x < LOGICAL_GRID_SIZE
    )


def _compose_tail_frame(layers: TailLayers, angle: int) -> np.ndarray:
    frame = layers.baseplate.copy()
    moved = _rotate_tail_layer(layers, angle)
    moved_visible = moved[:, :, 3] > 0
    frame[moved_visible] = moved[moved_visible]
    frame[layers.seam] = layers.source[layers.seam]
    return frame


def _rotate_tail_layer(layers: TailLayers, angle: int) -> np.ndarray:
    layer = Image.fromarray(layers.tail_pixels, "RGBA")
    rotated = layer.rotate(
        angle,
        resample=Image.Resampling.NEAREST,
        center=layers.root,
        fillcolor=(0, 0, 0, 0),
    )
    return np.asarray(rotated, dtype=np.uint8)


__all__ = ["TAIL_WAG_ANGLES", "make_tail_wag_frames"]

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

import math
from collections import deque
from dataclasses import dataclass
from typing import Final

import numpy as np
from PIL import Image, ImageDraw

type Point = tuple[int, int]

LOGICAL_GRID_SIZE: Final = 160
ROOT_SEAM_LENGTH: Final = 7
TAIL_WAVE_FRAME_COUNT: Final = 25
TAIL_WAVE_PEAK_PIXELS: Final = 7
TAIL_WAVE_K: Final = 1.5 * math.pi


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
    """Build loop-closing tail-wave frames (traveling wave along the tail)."""
    layers = _build_tail_layers(source, mask_points, root)
    tail_alpha = layers.tail_pixels[:, :, 3] > 0
    distance = _geodesic_distance(tail_alpha, root)
    return tuple(
        _compose_wave_frame(layers, distance, 2 * math.pi * i / (TAIL_WAVE_FRAME_COUNT - 1))
        for i in range(TAIL_WAVE_FRAME_COUNT)
    )


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


def _geodesic_distance(tail_alpha: np.ndarray, root: Point) -> np.ndarray:
    """BFS 测地距离：每个尾巴像素到 root 的沿尾巴最短步数（≈弧长）。"""
    distance = np.full((LOGICAL_GRID_SIZE, LOGICAL_GRID_SIZE), -1, dtype=np.int32)
    root_y, root_x = root[1], root[0]
    if not tail_alpha[root_y, root_x]:
        ys, xs = np.where(tail_alpha)
        if len(ys) == 0:
            return distance
        squared = (xs - root[0]) ** 2 + (ys - root[1]) ** 2
        closest = int(np.argmin(squared))
        root_y, root_x = int(ys[closest]), int(xs[closest])
    distance[root_y, root_x] = 0
    queue = deque([(root_y, root_x)])
    while queue:
        y, x = queue.popleft()
        for next_y, next_x in ((y - 1, x), (y, x - 1), (y, x + 1), (y + 1, x)):
            if (
                0 <= next_y < LOGICAL_GRID_SIZE
                and 0 <= next_x < LOGICAL_GRID_SIZE
                and tail_alpha[next_y, next_x]
                and distance[next_y, next_x] < 0
            ):
                distance[next_y, next_x] = distance[y, x] + 1
                queue.append((next_y, next_x))
    return distance


def _wave_tail(layers: TailLayers, distance: np.ndarray, phase: float) -> np.ndarray:
    """逐列上下整数位移（列内不撕裂）+ 根部 seam 锚死。"""
    tail_alpha = layers.tail_pixels[:, :, 3] > 0
    max_distance = int(distance.max())
    if max_distance <= 0:
        return layers.tail_pixels.copy()
    moved = np.zeros_like(layers.tail_pixels)
    for x in range(LOGICAL_GRID_SIZE):
        column = tail_alpha[:, x]
        if not column.any():
            continue
        s = float(distance[column, x].mean()) / max_distance
        amplitude = TAIL_WAVE_PEAK_PIXELS * (
            np.clip(s, 0, 1) ** 2 * (3 - 2 * np.clip(s, 0, 1))
        )
        dy = int(round(amplitude * math.sin(phase - TAIL_WAVE_K * s)))
        ys = np.where(column)[0]
        new_ys = ys + dy
        valid = (new_ys >= 0) & (new_ys < LOGICAL_GRID_SIZE)
        moved[new_ys[valid], x] = layers.tail_pixels[ys[valid], x]
    moved[layers.seam] = layers.tail_pixels[layers.seam]
    return moved


def _compose_wave_frame(layers: TailLayers, distance: np.ndarray, phase: float) -> np.ndarray:
    frame = layers.baseplate.copy()
    moved = _wave_tail(layers, distance, phase)
    moved_visible = moved[:, :, 3] > 0
    frame[moved_visible] = moved[moved_visible]
    frame[layers.seam] = layers.source[layers.seam]
    return frame


__all__ = ["TAIL_WAVE_FRAME_COUNT", "TAIL_WAVE_PEAK_PIXELS", "make_tail_wag_frames"]

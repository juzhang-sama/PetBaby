from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Literal, Self

import numpy as np
from PIL import Image, ImageDraw
from pydantic import BaseModel, ConfigDict, Field, model_validator
from pydantic_core import PydanticCustomError

from 中等简约尾巴 import make_tail_wag_frames

type Point = tuple[int, int]
type Rect = tuple[int, int, int, int]

LOGICAL_GRID_SIZE = 160
VISIBLE_COLOR_LIMIT = 24
BREATH_FRAME_COUNT = 49
BREATH_PEAK_PIXELS = 2
BREATH_HEAD_SPAN = 0.30     # 头脸段（breathZone 上 30%）不动
BREATH_HEAD_RISE = 0.10     # 头脸→胸腹 过渡带宽度（占 breathZone 高度比例）
BREATH_FOOT_FALL = 0.15     # 腿脚 过渡带宽度


@dataclass(frozen=True, slots=True)
class MotionError(Exception):
    message: str

    def __str__(self) -> str:
        return self.message


class TailAnnotation(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid", populate_by_name=True)

    enabled: bool
    root: Point | None
    mask: tuple[Point, ...]
    disabled_reason: str | None = Field(alias="disabledReason")

    @model_validator(mode="after")
    def check_contract(self) -> Self:
        for point in self.mask:
            _check_point(point, "tail.mask")
        if self.root is not None:
            _check_point(self.root, "tail.root")
        if self.enabled and (self.root is None or len(self.mask) < 3):
            raise PydanticCustomError(
                "tail_geometry_missing",
                "enabled tail requires root and at least three mask points",
            )
        if self.enabled and self.disabled_reason is not None:
            raise PydanticCustomError(
                "tail_disable_reason_conflict",
                "enabled tail cannot have disabledReason",
            )
        if not self.enabled and not self.disabled_reason:
            raise PydanticCustomError(
                "tail_disable_reason_missing",
                "disabled tail requires disabledReason",
            )
        return self

    @property
    def bounds(self) -> Rect:
        if not self.mask:
            raise MotionError("tail mask has no bounds")
        xs = tuple(point[0] for point in self.mask)
        ys = tuple(point[1] for point in self.mask)
        return min(xs), min(ys), max(xs), max(ys)


class MotionAnnotation(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid", populate_by_name=True)

    logical_grid_size: Literal[160] = Field(alias="logicalGridSize")
    eyes: dict[Literal["left", "right"], Rect]
    breath_zone: Rect = Field(alias="breathZone")
    ground_anchors: tuple[Rect, ...] = Field(alias="groundAnchors")
    tail: TailAnnotation

    @model_validator(mode="after")
    def check_contract(self) -> Self:
        if set(self.eyes) != {"left", "right"}:
            raise PydanticCustomError(
                "eye_keys_invalid", "eyes must contain exactly left and right"
            )
        for name, rect in self.eyes.items():
            _check_rect(rect, f"eyes.{name}", needs_margin=True)
        _check_rect(self.breath_zone, "breathZone")
        if not self.ground_anchors:
            raise PydanticCustomError(
                "ground_anchor_missing", "groundAnchors must not be empty"
            )
        for rect in self.ground_anchors:
            _check_rect(rect, "groundAnchors")
        return self

    @property
    def eye_bounds(self) -> Rect:
        left = self.eyes["left"]
        right = self.eyes["right"]
        return (
            min(left[0], right[0]),
            min(left[1], right[1]),
            max(left[2], right[2]),
            max(left[3], right[3]),
        )


@dataclass(frozen=True, slots=True)
class MotionAudit:
    action_id: str
    frame_count: int
    loop_closed: bool
    ground_anchor_changed_pixels: int
    maximum_visible_colors: int
    peak_changed_pixels: int


@dataclass(frozen=True, slots=True)
class ActionAuditSpec:
    action_id: str
    annotation: MotionAnnotation
    target: Rect


def make_breath(source: np.ndarray, annotation: MotionAnnotation) -> tuple[np.ndarray, ...]:
    _check_source(source)
    return tuple(
        _inflate_breath_zone(source, annotation, level)
        for level in _cos_breath_levels(BREATH_FRAME_COUNT, BREATH_PEAK_PIXELS)
    )


def make_blink(source: np.ndarray, annotation: MotionAnnotation) -> tuple[np.ndarray, ...]:
    _check_source(source)
    return tuple(
        _close_eyes(source, annotation.eyes, level)
        for level in (0.0, 0.5, 1.0, 0.5, 0.0)
    )


def make_tail_wag(
    source: np.ndarray, annotation: MotionAnnotation
) -> tuple[np.ndarray, ...]:
    _check_source(source)
    if not annotation.tail.enabled or annotation.tail.root is None:
        raise MotionError("tail wag requires an enabled tail annotation")
    return make_tail_wag_frames(source, annotation.tail.mask, annotation.tail.root)


def audit_action(
    spec: ActionAuditSpec,
    source: np.ndarray,
    frames: tuple[np.ndarray, ...],
) -> MotionAudit:
    if not frames:
        raise MotionError(f"{spec.action_id} produced no frames")
    loop_closed = np.array_equal(frames[0], frames[-1])
    if not loop_closed:
        raise MotionError(f"{spec.action_id} loop does not close")
    for frame in frames:
        _check_source(frame)
    anchor_changed = max(
        sum(_changed_pixels(source, frame, rect) for rect in spec.annotation.ground_anchors)
        for frame in frames
    )
    maximum_colors = max(_visible_color_count(frame) for frame in frames)
    peak_changed = max(_changed_pixels(source, frame, spec.target) for frame in frames)
    if anchor_changed != 0:
        raise MotionError(
            f"{spec.action_id} changed {anchor_changed} ground-anchor pixels"
        )
    if maximum_colors > VISIBLE_COLOR_LIMIT:
        raise MotionError(f"{spec.action_id} exceeds the 24-color limit")
    if peak_changed == 0:
        raise MotionError(f"{spec.action_id} has no visible target movement")
    return MotionAudit(
        action_id=spec.action_id,
        frame_count=len(frames),
        loop_closed=loop_closed,
        ground_anchor_changed_pixels=anchor_changed,
        maximum_visible_colors=maximum_colors,
        peak_changed_pixels=peak_changed,
    )


def _cos_breath_levels(n_frames: int, peak: int) -> tuple[float, ...]:
    """余弦缓动的连续膨胀量（浮点不取整），首尾波谷 0、中间峰值 peak。"""
    return tuple(
        peak * (1 - math.cos(2 * math.pi * i / (n_frames - 1))) / 2
        for i in range(n_frames)
    )


def _smoothstep(t: float) -> float:
    """0..1 的平滑缓动（smoothstep），两端斜率 0。"""
    t = min(1.0, max(0.0, t))
    return t * t * (3 - 2 * t)


def _breath_weight(normalized_y: float) -> float:
    """平台形权重：头脸 0 → 过渡带平滑升到 1 → 胸腹主体 flat=1 → 腿脚过渡带平滑降到 0。

    主体均匀膨胀（消除"中间尖峰向上下衰减"的波浪感），只在两端边界柔和收口。
    """
    if normalized_y < BREATH_HEAD_SPAN:
        return 0.0
    if normalized_y < BREATH_HEAD_SPAN + BREATH_HEAD_RISE:
        return _smoothstep((normalized_y - BREATH_HEAD_SPAN) / BREATH_HEAD_RISE)
    foot_start = 1.0 - BREATH_FOOT_FALL
    if normalized_y < foot_start:
        return 1.0
    if normalized_y < 1.0:
        return 1.0 - _smoothstep((normalized_y - foot_start) / BREATH_FOOT_FALL)
    return 0.0


def _tail_mask(annotation: MotionAnnotation, size: int = LOGICAL_GRID_SIZE) -> np.ndarray:
    """尾巴多边形掩码（膨胀时排除，避免拉宽尾巴根）。"""
    img = Image.new("L", (size, size), 0)
    ImageDraw.Draw(img).polygon(annotation.tail.mask, fill=1)
    return np.asarray(img) > 0


def _contiguous_segments(mask: np.ndarray) -> list[tuple[int, int]]:
    """一维布尔掩码的连续 True 段 [(start, end), ...]（闭区间）。"""
    segs: list[tuple[int, int]] = []
    idxs = np.nonzero(mask)[0]
    if len(idxs) == 0:
        return segs
    start = int(idxs[0])
    prev = int(idxs[0])
    for i in idxs[1:]:
        if int(i) != prev + 1:
            segs.append((start, prev))
            start = int(i)
        prev = int(i)
    segs.append((start, prev))
    return segs


def _inflate_breath_zone(source: np.ndarray, annotation: MotionAnnotation, k: float) -> np.ndarray:
    """横向重采样拉伸：每行身体段拉宽，内部花纹跟着扩散（膨胀感的来源）。

    左右对称（每侧扩 half 像素）、头脸不动、尾巴排除、腿脚不动。
    k 为浮点膨胀量（余弦曲线），每行 half 按 1px 粒度取整，得到多档过渡。
    """
    if k == 0:
        return source.copy()
    x0, y0, x1, y1 = annotation.breath_zone
    tail = _tail_mask(annotation)
    frame = source.copy()
    span = max(1, y1 - y0)
    for y in range(y0, y1 + 1):
        weight = _breath_weight((y - y0) / span)
        half = int(round(k * weight))  # 每侧扩 half，左右对称
        if half <= 0:
            continue
        # 身体段 = breathZone x 范围内可见，且排除尾巴 mask
        alpha = frame[y, x0 : x1 + 1, 3] > 0
        alpha &= ~tail[y, x0 : x1 + 1]
        for sl, sr in _contiguous_segments(alpha):
            seg_n = sr - sl + 1
            if seg_n < 3:
                continue
            nl = max(0, sl - half)
            nr = min(x1 - x0, sr + half)
            new_len = nr - nl + 1
            if new_len <= seg_n:
                continue
            # 最近邻重采样：内部花纹像素跟着重新分布（膨胀感关键）
            idx = np.round(
                np.arange(new_len) * (seg_n - 1) / max(1, new_len - 1)
            ).astype(int)
            frame[y, x0 + nl : x0 + nr + 1] = frame[y, x0 + sl : x0 + sr + 1][idx]
    return frame


def _close_eyes(
    source: np.ndarray,
    eyes: dict[Literal["left", "right"], Rect],
    level: float,
) -> np.ndarray:
    if level == 0:
        return source.copy()
    frame = source.copy()
    for x0, y0, x1, y1 in eyes.values():
        height = y1 - y0 + 1
        closed_rows = max(1, min(height, round(height * level)))
        upper = closed_rows // 2
        lower = closed_rows - upper
        upper_y = max(0, y0 - 2)
        lower_y = min(LOGICAL_GRID_SIZE - 1, y1 + 2)
        sample_x0 = max(0, x0 - 2)
        sample_x1 = min(LOGICAL_GRID_SIZE, x1 + 3)
        upper_color = _dominant_visible_color(
            source[upper_y : upper_y + 1, sample_x0:sample_x1]
        )
        lower_color = _dominant_visible_color(
            source[lower_y : lower_y + 1, sample_x0:sample_x1]
        )
        frame[y0 : y0 + upper, x0 : x1 + 1] = upper_color
        frame[y1 + 1 - lower : y1 + 1, x0 : x1 + 1] = lower_color
        if level == 1.0:
            eye = source[y0 : y1 + 1, x0 : x1 + 1].reshape(-1, 4)
            visible = eye[eye[:, 3] > 0]
            lid_color = visible[np.argmin(visible[:, :3].sum(axis=1))]
            frame[(y0 + y1) // 2, x0 + 1 : x1] = lid_color
    return frame


def _dominant_visible_color(pixels: np.ndarray) -> np.ndarray:
    visible = pixels.reshape(-1, 4)
    visible = visible[visible[:, 3] > 0]
    if not visible.size:
        raise MotionError("eye lid sampling requires visible pixels")
    colors, counts = np.unique(visible, axis=0, return_counts=True)
    return colors[int(np.argmax(counts))]


def _changed_pixels(first: np.ndarray, second: np.ndarray, rect: Rect) -> int:
    x0, y0, x1, y1 = rect
    delta = first[y0 : y1 + 1, x0 : x1 + 1] != second[y0 : y1 + 1, x0 : x1 + 1]
    return int(np.any(delta, axis=2).sum())


def _visible_color_count(frame: np.ndarray) -> int:
    visible = frame[frame[:, :, 3] > 0, :3]
    return int(np.unique(visible, axis=0).shape[0])


def _check_source(source: np.ndarray) -> None:
    if source.shape != (LOGICAL_GRID_SIZE, LOGICAL_GRID_SIZE, 4):
        raise MotionError("motion source must be a 160x160 RGBA array")
    if source.dtype != np.uint8:
        raise MotionError("motion source must use uint8 pixels")


def _check_point(point: Point, label: str) -> None:
    if any(value < 0 or value >= LOGICAL_GRID_SIZE for value in point):
        raise PydanticCustomError("point_out_of_bounds", f"{label} must stay in 0..159")


def _check_rect(rect: Rect, label: str, *, needs_margin: bool = False) -> None:
    left, top, right, bottom = rect
    if any(value < 0 or value >= LOGICAL_GRID_SIZE for value in rect):
        raise PydanticCustomError("rect_out_of_bounds", f"{label} must stay in 0..159")
    if left >= right or top >= bottom:
        raise PydanticCustomError("rect_reversed", f"{label} must have positive area")
    if needs_margin and (top == 0 or bottom == LOGICAL_GRID_SIZE - 1):
        raise PydanticCustomError("eye_margin_missing", f"{label} needs one vertical margin pixel")


__all__ = [
    "ActionAuditSpec",
    "MotionAnnotation",
    "MotionAudit",
    "MotionError",
    "Point",
    "Rect",
    "audit_action",
    "make_blink",
    "make_breath",
    "make_tail_wag",
]

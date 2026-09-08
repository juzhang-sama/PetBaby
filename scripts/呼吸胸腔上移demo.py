# -*- coding: utf-8 -*-
"""呼吸 demo（横向拉伸 + 内部花纹跟着扩散）。

回到搜索方案前的版本（_inflate_breath_zone 横向整数拉伸），并修复当时的 bug：

核心认知（用户拍板）：
  膨胀感 = 内部花纹跟着身体一起扩散。所以必须用「横向重采样拉伸」——
  每行身体段从 n 像素最近邻重采样拉宽到 n+d 像素，内部花纹像素跟着重新分布，
  而不是只动外圈（uniform scale / 边缘复制都不对）。

修复：
  1. 对称：每侧扩 half 像素（half = round(k*weight)），左右严格相等，
     不再用 d//2 / d-d//2（奇偶不对称，"右侧先动"的根源）。
  2. 范围自适应：直接用每只宠物的 breathZone 标注（[x0,y0,x1,y1]），
     不再硬编码锚点。
  3. 尾巴排除：尾巴根在 breathZone 内，用 tail.mask 排除，避免被拉宽。
  4. 头脸不动：breathZone 上 30% weight=0（沿用原 _breath_weight）。
  5. 频率：慢周期 49 帧 × 122ms ≈ 6 秒，余弦缓动，峰值每侧 2px。
"""
from __future__ import annotations

import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

sys.path.insert(0, str(Path(__file__).resolve().parent))

from 中等简约产物 import load_logical_rgba  # noqa: E402
from 中等简约动作 import MotionAnnotation  # noqa: E402

SCALE = 6
OUT_ROOT = Path(__file__).resolve().parents[1] / "output" / "呼吸胸腔上移demo-2026-08-22"

FRAME_COUNT = 49
PEAK_PIXELS = 2        # 峰值每侧扩 2px（3px 降 30%→2.1 取整 2px）
FRAME_DURATION_MS = 85  # 122ms 快 30%→85ms，周期 49*85≈4.2 秒
HEAD_SPAN = 0.30       # 头脸段（breathZone 上 30%）不动
HEAD_RISE = 0.10       # 头脸→胸腹 过渡带宽度（占 breathZone 高度比例）
FOOT_FALL = 0.15       # 腿脚 过渡带宽度


def _smoothstep(t: float) -> float:
    """0..1 的平滑缓动（smoothstep），两端斜率 0。"""
    t = min(1.0, max(0.0, t))
    return t * t * (3 - 2 * t)


def _breath_weight(normalized_y: float) -> float:
    """平台形权重：头脸 0 → 过渡带平滑升到 1 → 胸腹主体 flat=1 → 腿脚过渡带平滑降到 0。

    主体均匀膨胀（消除"中间尖峰向上下衰减"的波浪感），只在两端边界柔和收口。
    """
    if normalized_y < HEAD_SPAN:
        return 0.0
    if normalized_y < HEAD_SPAN + HEAD_RISE:
        return _smoothstep((normalized_y - HEAD_SPAN) / HEAD_RISE)
    foot_start = 1.0 - FOOT_FALL
    if normalized_y < foot_start:
        return 1.0
    if normalized_y < 1.0:
        return 1.0 - _smoothstep((normalized_y - foot_start) / FOOT_FALL)
    return 0.0


def _tail_mask(annotation: MotionAnnotation, size: int = 160) -> np.ndarray:
    img = Image.new("L", (size, size), 0)
    ImageDraw.Draw(img).polygon(annotation.tail.mask, fill=1)
    return np.asarray(img) > 0


def _cos_levels(n_frames: int, peak: int) -> tuple[float, ...]:
    return tuple(
        peak * (1 - math.cos(2 * math.pi * i / (n_frames - 1))) / 2
        for i in range(n_frames)
    )


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


def _inflate_breath(source: np.ndarray, annotation: MotionAnnotation, k: float) -> np.ndarray:
    """横向重采样拉伸：每行身体段拉宽，内部花纹跟着扩散。左右对称、头脸/尾巴/腿脚不动。"""
    x0, y0, x1, y1 = annotation.breath_zone
    tail = _tail_mask(annotation)
    frame = source.copy()
    width = source.shape[1]
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
            # sl/sr 是相对 x0 的偏移
            seg_n = sr - sl + 1
            if seg_n < 3:
                continue
            nl = max(0, sl - half)
            nr = min(x1 - x0, sr + half)
            new_len = nr - nl + 1
            if new_len <= seg_n:
                continue
            # 最近邻重采样：内部花纹像素跟着重新分布（这是"膨胀感"的关键）
            idx = np.round(
                np.arange(new_len) * (seg_n - 1) / max(1, new_len - 1)
            ).astype(int)
            frame[y, x0 + nl : x0 + nr + 1] = frame[y, x0 + sl : x0 + sr + 1][idx]
    return frame


def make_breath_frames(
    source: np.ndarray, annotation: MotionAnnotation
) -> tuple[np.ndarray, ...]:
    return tuple(
        _inflate_breath(source, annotation, k)
        for k in _cos_levels(FRAME_COUNT, PEAK_PIXELS)
    )


def _peak_frame(frames, base: np.ndarray) -> tuple[np.ndarray, int]:
    changed = [int(np.any(f != base, axis=2).sum()) for f in frames]
    idx = int(np.argmax(changed))
    return frames[idx], changed[idx]


def _upscale(img: np.ndarray) -> Image.Image:
    return Image.fromarray(img, "RGBA").resize(
        (160 * SCALE, 160 * SCALE), Image.Resampling.NEAREST
    )


def _gif(frames, path: Path, duration: int) -> None:
    size = 160 * SCALE
    review = [
        Image.alpha_composite(
            Image.new("RGBA", (160, 160), (236, 234, 229, 255)), Image.fromarray(f, "RGBA")
        ).resize((size, size), Image.Resampling.NEAREST).convert("P", palette=Image.Palette.ADAPTIVE)
        for f in frames
    ]
    review[0].save(
        path, save_all=True, append_images=review[1:], duration=duration, loop=0, disposal=2
    )


def main() -> None:
    base = Path(__file__).resolve().parents[1] / "output" / "中等简约像素标准验收-2026-08-21"
    OUT_ROOT.mkdir(parents=True, exist_ok=True)

    pet_ids = ["01-longhair-black-white", "02-round-tabby", "03-sleek-black"]
    for pid in pet_ids:
        src = base / pid / "母版.png"
        annotation = MotionAnnotation.model_validate_json(
            (base / "annotations" / f"{pid}.json").read_text(encoding="utf-8")
        )
        logical = load_logical_rgba(src)

        frames = list(make_breath_frames(logical, annotation))
        peak, _ = _peak_frame(frames, logical)

        diff = np.any(peak != logical, axis=2)
        vis = np.asarray(Image.fromarray(logical, "RGBA").convert("RGB")).copy()
        vis[diff] = (255, 60, 50)
        Image.fromarray(vis).resize(
            (160 * SCALE, 160 * SCALE), Image.Resampling.NEAREST
        ).convert("RGB").save(OUT_ROOT / f"{pid}-动区标注.png")

        _gif(frames, OUT_ROOT / f"{pid}-呼吸.gif", FRAME_DURATION_MS)
        print(f"{pid}: 帧数={len(frames)} 周期={FRAME_COUNT*FRAME_DURATION_MS}ms")

    print(f"输出目录: {OUT_ROOT}")


if __name__ == "__main__":
    main()

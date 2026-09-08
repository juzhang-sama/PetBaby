# -*- coding: utf-8 -*-
"""呼吸膨胀 demo：对比旧「平移」vs 正式「整数拉伸膨胀」（直接调用 make_breath，保证一致）。

在 160 逻辑网格上做整数像素操作，NEAREST 放大到 1024，保持像素硬边。
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))

from 中等简约产物 import load_logical_rgba  # noqa: E402
from 中等简约动作 import make_breath, MotionAnnotation  # noqa: E402

PET_ID = "01-longhair-black-white"
SCALE = 6
OUT_ROOT = Path(__file__).resolve().parents[1] / "output" / "呼吸膨胀demo-2026-08-21"


# 旧算法：整块平移（rise 像素），底边补齐
def _shift_breath_zone(source: np.ndarray, rect, rise: int) -> np.ndarray:
    if rise == 0:
        return source.copy()
    x0, y0, x1, y1 = rect
    frame = source.copy()
    region = source[y0 : y1 + 1, x0 : x1 + 1]
    frame[y0 : y1 + 1 - rise, x0 : x1 + 1] = region[rise:]
    frame[y1 + 1 - rise : y1 + 1, x0 : x1 + 1] = region[-1:]
    return frame


def _peak_frame(frames: list[np.ndarray], base: np.ndarray) -> tuple[np.ndarray, int]:
    changed = [int(np.any(f != base, axis=2).sum()) for f in frames]
    idx = int(np.argmax(changed))
    return frames[idx], changed[idx]


def _upscale(img: np.ndarray) -> Image.Image:
    return Image.fromarray(img, "RGBA").resize(
        (160 * SCALE, 160 * SCALE), Image.Resampling.NEAREST
    )


def main() -> None:
    base = Path(__file__).resolve().parents[1] / "output" / "中等简约像素标准验收-2026-08-21"
    src = base / PET_ID / "母版.png"
    annotation_path = base / "annotations" / f"{PET_ID}.json"
    logical = load_logical_rgba(src)
    annotation = MotionAnnotation.model_validate_json(annotation_path.read_text(encoding="utf-8"))
    OUT_ROOT.mkdir(parents=True, exist_ok=True)

    # 旧平移帧
    old_frames = [_shift_breath_zone(logical, annotation.breath_zone, r) for r in (0, 1, 1, 2, 2, 1, 1, 0)]
    # 正式呼吸帧（直接调用 make_breath，保证与桌面运行时一致）
    new_frames = list(make_breath(logical, annotation))

    old_peak, old_chg = _peak_frame(old_frames, logical)
    new_peak, new_chg = _peak_frame(new_frames, logical)

    # 保存正式帧序列
    frame_dir = OUT_ROOT / "frames"
    frame_dir.mkdir(exist_ok=True)
    for i, f in enumerate(new_frames):
        _upscale(f).save(frame_dir / f"f{i:02d}.png")

    # 拼对比图：母版 | 旧平移峰值 | 正式膨胀峰值
    canvas = Image.new("RGBA", (160 * SCALE * 3, 160 * SCALE), (244, 244, 242, 255))
    for i, img in enumerate([logical, old_peak, new_peak]):
        canvas.alpha_composite(_upscale(img), (i * 160 * SCALE, 0))
    canvas.convert("RGB").save(OUT_ROOT / "对比.png")

    # GIF（正式算法）
    review = [Image.alpha_composite(
        Image.new("RGBA", (160, 160), (236, 234, 229, 255)), Image.fromarray(f, "RGBA")
    ).convert("P", palette=Image.Palette.ADAPTIVE) for f in new_frames]
    review[0].save(OUT_ROOT / "呼吸膨胀.gif", save_all=True, append_images=review[1:],
                   duration=122, loop=0, disposal=2)

    print(f"正式呼吸帧数: {len(new_frames)}")
    print(f"旧平移峰值变化: {old_chg}px")
    print(f"正式膨胀峰值变化: {new_chg}px")
    # 头部段（breath_zone 上 30%）是否变化
    x0, y0, x1, y1 = annotation.breath_zone
    head_cut = y0 + int((y1 - y0) * 0.30)
    head_chg = int(np.any(new_peak[y0:head_cut] != logical[y0:head_cut], axis=2).sum())
    print(f"头部变化: {head_chg}px（应=0）")
    print(f"输出目录: {OUT_ROOT}")


if __name__ == "__main__":
    main()

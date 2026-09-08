# -*- coding: utf-8 -*-
"""尾巴波浪 demo：上下摆动 + 一个 S 弧 + 尖端 6~8px。

复用现有分层（中等简约尾巴._build_tail_layers：挖尾巴→修复身体 baseplate + 尾巴像素 + 根部 seam），
只把"刚体旋转"替换为"逐像素上下波浪位移"。

核心：
  位移(s,t) = A(s) · sin(ωt − k·s)
  s = 尾巴像素到 root 的测地距离（BFS 沿尾巴连通域），天然处理卷曲尾巴。
  A(s) = peak · smoothstep(s)，根 0 → 尖 peak。
  k = 1.5π（一个 S 弧）。
"""
from __future__ import annotations

import math
import sys
from collections import deque
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

sys.path.insert(0, str(Path(__file__).resolve().parent))

from 中等简约产物 import load_logical_rgba  # noqa: E402
from 中等简约动作 import MotionAnnotation  # noqa: E402
from 中等简约尾巴 import _build_tail_layers  # noqa: E402

SCALE = 6
OUT_ROOT = Path(__file__).resolve().parents[1] / "output" / "尾巴波浪demo-2026-08-22"
SIZE = 160

FRAME_COUNT = 25
PEAK_PIXELS = 7          # 尖端最大摆 7px（6~8 区间）
K_WAVE = 1.5 * math.pi   # 波数：一个 S 弧
FRAME_DURATION_MS = 80   # 周期 25*80=2 秒


def _geodesic_distance(tail_alpha: np.ndarray, root) -> np.ndarray:
    """BFS 沿尾巴连通域，算每个像素到 root 的测地距离（≈弧长）。root=(x,y)。"""
    dist = np.full((SIZE, SIZE), -1, dtype=np.int32)
    ry, rx = root[1], root[0]
    if not tail_alpha[ry, rx]:
        # root 不在尾巴像素上（可能被 seam 判定为身体），找最近的尾巴像素
        ys, xs = np.where(tail_alpha)
        if len(ys) == 0:
            return dist
        d2 = (xs - root[0]) ** 2 + (ys - root[1]) ** 2
        k = int(np.argmin(d2))
        ry, rx = int(ys[k]), int(xs[k])
    dist[ry, rx] = 0
    q = deque([(ry, rx)])
    while q:
        y, x = q.popleft()
        for ny, nx in ((y - 1, x), (y, x - 1), (y, x + 1), (y + 1, x)):
            if 0 <= ny < SIZE and 0 <= nx < SIZE and tail_alpha[ny, nx] and dist[ny, nx] < 0:
                dist[ny, nx] = dist[y, x] + 1
                q.append((ny, nx))
    return dist


def _wave_tail(layers, dist: np.ndarray, phi: float) -> np.ndarray:
    """逐列上下整数位移（列内不撕裂）+ 根部 seam 锚死。"""
    tail_alpha = layers.tail_pixels[:, :, 3] > 0
    smax = int(dist.max())
    if smax <= 0:
        return layers.tail_pixels.copy()
    moved = np.zeros_like(layers.tail_pixels)
    for x in range(SIZE):
        col = tail_alpha[:, x]
        if not col.any():
            continue
        s_col = float(dist[col, x].mean()) / smax
        amp = PEAK_PIXELS * (np.clip(s_col, 0, 1) ** 2 * (3 - 2 * np.clip(s_col, 0, 1)))
        dy = int(round(amp * math.sin(phi - K_WAVE * s_col)))
        ys = np.where(col)[0]
        new_ys = ys + dy
        valid = (new_ys >= 0) & (new_ys < SIZE)
        moved[new_ys[valid], x] = layers.tail_pixels[ys[valid], x]
    # 根部 seam 锚死
    moved[layers.seam] = layers.tail_pixels[layers.seam]
    return moved


def _compose(layers, dist: np.ndarray, phi: float) -> np.ndarray:
    frame = layers.baseplate.copy()
    moved = _wave_tail(layers, dist, phi)
    moved_visible = moved[:, :, 3] > 0
    frame[moved_visible] = moved[moved_visible]
    frame[layers.seam] = layers.source[layers.seam]
    return frame


def make_tail_wave_frames(
    source: np.ndarray, annotation: MotionAnnotation
) -> tuple[np.ndarray, ...]:
    layers = _build_tail_layers(source, annotation.tail.mask, annotation.tail.root)
    tail_alpha = layers.tail_pixels[:, :, 3] > 0
    dist = _geodesic_distance(tail_alpha, annotation.tail.root)
    return tuple(
        _compose(layers, dist, 2 * math.pi * i / (FRAME_COUNT - 1))
        for i in range(FRAME_COUNT)
    )


def _upscale(img: np.ndarray) -> Image.Image:
    return Image.fromarray(img, "RGBA").resize(
        (SIZE * SCALE, SIZE * SCALE), Image.Resampling.NEAREST
    )


def _gif(frames, path: Path, duration: int) -> None:
    size = SIZE * SCALE
    review = [
        Image.alpha_composite(
            Image.new("RGBA", (SIZE, SIZE), (236, 234, 229, 255)), Image.fromarray(f, "RGBA")
        ).resize((size, size), Image.Resampling.NEAREST).convert("P", palette=Image.Palette.ADAPTIVE)
        for f in frames
    ]
    review[0].save(path, save_all=True, append_images=review[1:], duration=duration, loop=0, disposal=2)


def main() -> None:
    base = Path(__file__).resolve().parents[1] / "output" / "中等简约像素标准验收-2026-08-21"
    OUT_ROOT.mkdir(parents=True, exist_ok=True)

    pet_ids = ["01-longhair-black-white", "02-round-tabby", "03-sleek-black"]
    for pid in pet_ids:
        annotation = MotionAnnotation.model_validate_json(
            (base / "annotations" / f"{pid}.json").read_text(encoding="utf-8")
        )
        logical = load_logical_rgba(base / pid / "母版.png")

        frames = list(make_tail_wave_frames(logical, annotation))
        peak = frames[FRAME_COUNT // 4]

        diff = np.any(peak != logical, axis=2)
        vis = np.asarray(Image.fromarray(logical, "RGBA").convert("RGB")).copy()
        vis[diff] = (255, 60, 50)
        Image.fromarray(vis).resize(
            (SIZE * SCALE, SIZE * SCALE), Image.Resampling.NEAREST
        ).convert("RGB").save(OUT_ROOT / f"{pid}-动区标注.png")

        _gif(frames, OUT_ROOT / f"{pid}-尾巴波浪.gif", FRAME_DURATION_MS)
        print(f"{pid}: 帧数={len(frames)}")

    print(f"输出目录: {OUT_ROOT}")


if __name__ == "__main__":
    main()

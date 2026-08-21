"""像素逻辑网格动画 demo（M2 v2 路线验证）。

核心假设：像素动画的正确工作层是「逻辑像素网格」（本项目 512x512），
所有几何变换（旋转/缩放/位移）都在逻辑网格层做（NEAREST 硬边、接受阶梯），
再放大回成品分辨率（2048，NEAREST 保持像素块方形）。全程无羽化。

本 demo 验证两个关键问题：
  1. 硬边旋转/缩放在逻辑网格上的「阶梯」效果是否可接受（像素风美学）。
  2. 区域操作后与底图的「拼凑/重影」问题是否明显（用户担心重蹈 Live2D 覆辙）。

用法：python scripts/像素逻辑网格demo.py <body.png> [-o output_dir]
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image

SCRIPT_DIR = Path(__file__).resolve().parent
BACKEND_ROOT = SCRIPT_DIR.parent / "services" / "appearance-generation" / "src" / "photo_avatar_backend"
TEMPLATE_PATH = BACKEND_ROOT / "assets" / "pixel-style-v1" / "标准猫体模板.json"

LOGICAL_SIZE = 512  # 像素化目标网格（pixel_png._PIXELATE_TARGET_SIZE）
FULL_SIZE = 2048    # 成品图尺寸
FRAMES = 10
DURATION_MS = 100


def load_template() -> dict:
    return json.loads(TEMPLATE_PATH.read_text(encoding="utf-8"))


def alpha_bounds_px(img: Image.Image) -> tuple[int, int, int, int]:
    a = np.asarray(img.convert("RGBA"))[:, :, 3]
    ys, xs = np.where(a > 0)
    return int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())


def template_rect_to_logical(rect: dict, t_alpha: tuple, p_alpha: tuple) -> tuple[int, int, int, int]:
    """模板归一化矩形 -> 512 逻辑网格像素（以 alphaBounds 对齐）。"""
    tl, tt, tr, tb = t_alpha
    pl, pt, pr, pb = p_alpha
    sx = (tr - tl) or 1.0
    sy = (tb - tt) or 1.0
    left = (pl + (rect["left"] - tl) / sx * (pr - pl)) / FULL_SIZE * LOGICAL_SIZE
    top = (pt + (rect["top"] - tt) / sy * (pb - pt)) / FULL_SIZE * LOGICAL_SIZE
    right = (pl + (rect["right"] - tl) / sx * (pr - pl)) / FULL_SIZE * LOGICAL_SIZE
    bottom = (pt + (rect["bottom"] - tt) / sy * (pb - pt)) / FULL_SIZE * LOGICAL_SIZE
    return int(left), int(top), int(right), int(bottom)


def to_logical(img: Image.Image) -> Image.Image:
    """成品图 -> 512 逻辑网格（BOX 平均，与像素化一致）。"""
    return img.convert("RGBA").resize((LOGICAL_SIZE, LOGICAL_SIZE), Image.Resampling.BOX)


def to_full(grid: Image.Image) -> Image.Image:
    """逻辑网格 -> 2048 成品（NEAREST，像素块保持方形）。"""
    return grid.resize((FULL_SIZE, FULL_SIZE), Image.Resampling.NEAREST)


def replace_region(frame: np.ndarray, box: tuple, region_img: Image.Image) -> np.ndarray:
    """把变换后的逻辑网格区域硬切覆盖回帧（无羽化）。"""
    x0, y0, x1, y1 = box
    x0, y0 = max(0, x0), max(0, y0)
    x1, y1 = min(LOGICAL_SIZE, x1), min(LOGICAL_SIZE, y1)
    out = frame.copy()
    arr = np.asarray(region_img)
    rh, rw = arr.shape[:2]
    out[y0 : y0 + rh, x0 : x0 + rw] = arr[: min(rh, y1 - y0), : min(rw, x1 - x0)]
    return out


def grid_to_image(grid: np.ndarray) -> Image.Image:
    return Image.fromarray(grid.astype(np.uint8), "RGBA")


# ---------------------------------------------------------------- 动作帧


def breath_frames(logical: Image.Image, zone: tuple, amp: float, n: int) -> list[Image.Image]:
    base = np.asarray(logical)
    x0, y0, x1, y1 = zone
    w, h = x1 - x0, y1 - y0
    frames = []
    for i in range(n):
        scale = 1.0 + amp * math.sin(2 * math.pi * i / n)
        crop = logical.crop((x0, y0, x1, y1))
        new_h = max(1, int(h * scale))
        scaled = crop.resize((w, new_h), Image.Resampling.NEAREST)
        frame = base.copy()
        # 顶部锚定覆盖（硬切）
        frame[y0 : y0 + new_h, x0 : x1] = np.asarray(scaled)[: new_h]
        frames.append(grid_to_image(frame))
    return frames


def tail_frames(logical: Image.Image, box: tuple, angle_deg: float, pivot_frac: tuple, n: int) -> list[Image.Image]:
    base = np.asarray(logical)
    x0, y0, x1, y1 = box
    frames = []
    for i in range(n):
        deg = angle_deg * math.sin(2 * math.pi * i / n)
        crop = logical.crop((x0, y0, x1, y1))
        rotated = crop.rotate(
            deg,
            resample=Image.Resampling.NEAREST,
            center=(pivot_frac[0] * (x1 - x0), pivot_frac[1] * (y1 - y0)),
            fillcolor=(0, 0, 0, 0),
        )
        frames.append(grid_to_image(replace_region(base, box, rotated)))
    return frames


def blink_frames(logical: Image.Image, eyes: list[tuple], n: int) -> list[Image.Image]:
    base = np.asarray(logical)
    frames = []
    for i in range(n):
        t = i / max(1, n - 1)
        # 两帧一眨，每眨：开 -> 闭 -> 开
        phase = (t * 2) % 1.0
        if phase < 0.2:
            ratio = 1.0
        elif phase < 0.5:
            ratio = 1.0 - (phase - 0.2) / 0.3 * 0.85
        elif phase < 0.7:
            ratio = 0.15
        else:
            ratio = 0.15 + (phase - 0.7) / 0.3 * 0.85
        frame = base.copy()
        for box in eyes:
            x0, y0, x1, y1 = box
            w, h = x1 - x0, y1 - y0
            if w <= 0 or h <= 0:
                continue
            crop = logical.crop((x0, y0, x1, y1))
            new_h = max(1, int(h * ratio))
            squashed = crop.resize((w, new_h), Image.Resampling.NEAREST)
            stretched = squashed.resize((w, h), Image.Resampling.NEAREST)
            frame[y0:y1, x0:x1] = np.asarray(stretched)
        frames.append(grid_to_image(frame))
    return frames


def save_gif(frames: list[Image.Image], out: Path) -> None:
    imgs = [f.convert("P", palette=Image.Palette.ADAPTIVE, colors=256) for f in frames]
    imgs[0].save(out, save_all=True, append_images=imgs[1:], duration=DURATION_MS, loop=0, disposal=2)


def main() -> int:
    parser = argparse.ArgumentParser(description="Pixel logical-grid motion demo (M2 v2)")
    parser.add_argument("body_png", type=Path)
    parser.add_argument("-o", "--output", type=Path, default=SCRIPT_DIR.parent / "output" / "pixel-motion-demo" / "v2")
    args = parser.parse_args()
    if not args.body_png.is_file():
        print(f"body.png not found: {args.body_png}", file=sys.stderr)
        return 2

    template = load_template()
    img = Image.open(args.body_png)
    logical = to_logical(img)
    p_alpha = alpha_bounds_px(img)
    t_alpha = tuple(template["space"]["alphaBounds"].values())

    zone = template_rect_to_logical(template["space"]["breathZone"], t_alpha, p_alpha)
    tail_part = next(p for p in template["parts"] if p["id"] == "tail")
    tail_box = template_rect_to_logical(tail_part["bounds"], t_alpha, p_alpha)
    tail_root = template_rect_to_logical(
        {
            "left": template["space"]["tailRoot"]["x"],
            "top": template["space"]["tailRoot"]["y"],
            "right": template["space"]["tailRoot"]["x"] + 0.001,
            "bottom": template["space"]["tailRoot"]["y"] + 0.001,
        },
        t_alpha, p_alpha,
    )
    px = (tail_root[0] - tail_box[0]) / max(1, tail_box[2] - tail_box[0])
    py = (tail_root[1] - tail_box[1]) / max(1, tail_box[3] - tail_box[1])
    pivot = (max(0, min(1, px)), max(0, min(1, py)))
    eyes = [
        template_rect_to_logical(template["space"]["eyes"][side]["bounds"], t_alpha, p_alpha)
        for side in ("left", "right")
    ]

    amp = float(template["amplitude"]["breath"]["max"])
    angle = float(template["amplitude"]["tailAngle"]["max"]) * 90

    args.output.mkdir(parents=True, exist_ok=True)
    save_gif(breath_frames(logical, zone, amp, FRAMES), args.output / "breath-grid.gif")
    save_gif(tail_frames(logical, tail_box, angle, pivot, FRAMES), args.output / "tail-grid.gif")
    save_gif(blink_frames(logical, eyes, FRAMES), args.output / "blink-grid.gif")

    # 拆层验证：用模板 parts 从逻辑层硬切，拼在一张图里看边界
    parts = template["parts"]
    canvas = Image.new("RGBA", (LOGICAL_SIZE * len(parts) + 20 * (len(parts) + 1), LOGICAL_SIZE), (255, 255, 255, 255))
    for index, part in enumerate(parts):
        box = template_rect_to_logical(part["bounds"], t_alpha, p_alpha)
        crop = logical.crop((max(0, box[0]), max(0, box[1]), min(LOGICAL_SIZE, box[2]), min(LOGICAL_SIZE, box[3])))
        canvas.paste(crop, (20 + index * (LOGICAL_SIZE + 20), 20))
        print(f"part {part['id']}: logical box={box} crop={crop.size}")
    canvas.save(args.output / "parts-split.png")
    print(f"wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

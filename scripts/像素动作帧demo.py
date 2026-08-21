"""M2 离线动作帧 demo 生成器。

用现有像素宠物 body.png + M1 标准猫体模板，按模板 parts/space 做区域形变，
生成三组动作 GIF（呼吸 / 眨眼 / 尾巴摆动），用于离线验证"部件级动作"效果。

用法：
  python scripts/像素动作帧demo.py <body.png> [-o output_dir]
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

DEFAULT_FRAMES = 8
DEFAULT_DURATION_MS = 110
PREVIEW_SIZE = 512  # GIF 预览降采样尺寸


# ---------------------------------------------------------------- 模板/坐标


def load_template() -> dict:
    return json.loads(TEMPLATE_PATH.read_text(encoding="utf-8"))


def alpha_bounds(img: Image.Image) -> tuple[int, int, int, int]:
    a = np.asarray(img.convert("RGBA"))[:, :, 3]
    ys, xs = np.where(a > 0)
    if len(xs) == 0:
        raise ValueError("image has no visible pixels")
    return int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())


def map_rect(
    t_rect: dict, t_alpha: tuple[float, float, float, float], p_alpha: tuple[int, int, int, int]
) -> tuple[int, int, int, int]:
    """模板归一化矩形 -> 实际像素矩形（以 alphaBounds 对齐映射）。"""
    tl, tt, tr, tb = t_alpha
    pl, pt, pr, pb = p_alpha
    span_x = (tr - tl) or 1.0
    span_y = (tb - tt) or 1.0
    left = pl + (t_rect["left"] - tl) / span_x * (pr - pl)
    top = pt + (t_rect["top"] - tt) / span_y * (pb - pt)
    right = pl + (t_rect["right"] - tl) / span_x * (pr - pl)
    bottom = pt + (t_rect["bottom"] - tt) / span_y * (pb - pt)
    return int(left), int(top), int(right), int(bottom)


def map_point(
    t_point: dict, t_alpha: tuple[float, float, float, float], p_alpha: tuple[int, int, int, int]
) -> tuple[int, int]:
    rect = {
        "left": t_point["x"],
        "top": t_point["y"],
        "right": t_point["x"] + 0.0001,
        "bottom": t_point["y"] + 0.0001,
    }
    x0, y0, _x1, _y1 = map_rect(rect, t_alpha, p_alpha)
    return x0, y0


# ---------------------------------------------------------------- 区域形变


def warp_region(
    frame: np.ndarray,
    box: tuple[int, int, int, int],
    transform,
    feather_frac: float = 0.08,
) -> np.ndarray:
    """对 box 区域应用 transform（接收并返回同尺寸 RGBA ndarray），羽化混合回 frame。"""
    x0, y0, x1, y1 = box
    h, w = y1 - y0, x1 - x0
    if h <= 0 or w <= 0:
        return frame
    pad_x = max(8, int(w * feather_frac))
    pad_y = max(8, int(h * feather_frac))
    cx0, cy0 = max(0, x0 - pad_x), max(0, y0 - pad_y)
    cx1, cy1 = min(frame.shape[1], x1 + pad_x), min(frame.shape[0], y1 + pad_y)
    region = frame[cy0:cy1, cx0:cx1].copy()
    warped = transform(region)
    if warped.shape != region.shape:
        raise ValueError("transform must preserve region size")
    # 羽化边缘：区域四周 feather_frac 宽度内 alpha 渐变到 0（与底图平滑过渡）
    rh, rw = region.shape[:2]
    fw = max(1, int(min(rw, rh) * feather_frac))
    feather = np.ones((rh, rw), dtype=np.float32)
    for i in range(fw):
        t = (i + 1) / (fw + 1)
        feather[i, :] = np.minimum(feather[i, :], t)
        feather[rh - 1 - i, :] = np.minimum(feather[rh - 1 - i, :], t)
        feather[:, i] = np.minimum(feather[:, i], t)
        feather[:, rw - 1 - i] = np.minimum(feather[:, rw - 1 - i], t)
    warp_alpha = warped[:, :, 3].astype(np.float32) / 255.0 * feather
    base_alpha = region[:, :, 3].astype(np.float32) / 255.0
    # 变换区域叠加：alpha 取两者最大值，RGB 在变换有像素处优先用变换结果
    combined_alpha = np.maximum(base_alpha, warp_alpha)
    out = np.zeros_like(region)
    mix = warp_alpha / np.maximum(combined_alpha, 1e-6)
    for c in range(3):
        out[:, :, c] = (
            region[:, :, c].astype(np.float32) * (1.0 - mix)
            + warped[:, :, c].astype(np.float32) * mix
        )
    out[:, :, 3] = combined_alpha * 255.0
    result = frame.copy()
    result[cy0:cy1, cx0:cx1] = out.astype(np.uint8)
    return result


def _scale_vertical(region: np.ndarray, scale_y: float, anchor: str = "top") -> np.ndarray:
    """垂直缩放内容但保持区域尺寸（缩放超出区域的部分裁剪掉）。"""
    h, w = region.shape[:2]
    new_h = max(1, int(round(h * scale_y)))
    im = Image.fromarray(region)
    resized = np.asarray(im.resize((w, new_h), Image.NEAREST))
    out = np.zeros_like(region)
    take = min(new_h, h)
    if anchor == "top":
        out[:take] = resized[:take]
    elif anchor == "bottom":
        out[h - take :] = resized[new_h - take :]
    else:  # center
        src_off = max(0, (new_h - h) // 2)
        dst_off = max(0, (h - new_h) // 2)
        count = min(take, h - dst_off, new_h - src_off)
        out[dst_off : dst_off + count] = resized[src_off : src_off + count]
    return out


def _squash(region: np.ndarray, close_ratio: float) -> np.ndarray:
    """垂直压缩内容（先压扁再拉回原尺寸，模拟闭眼）。"""
    h, w = region.shape[:2]
    new_h = max(1, int(h * close_ratio))
    im = Image.fromarray(region)
    squashed = np.asarray(im.resize((w, new_h), Image.NEAREST))
    return np.asarray(Image.fromarray(squashed).resize((w, h), Image.NEAREST))


def _rotate(region: np.ndarray, angle_deg: float, pivot_frac: tuple[float, float]) -> np.ndarray:
    h, w = region.shape[:2]
    im = Image.fromarray(region)
    rotated = im.rotate(
        angle_deg,
        resample=Image.BICUBIC,
        center=(pivot_frac[0] * w, pivot_frac[1] * h),
        fillcolor=(0, 0, 0, 0),
    )
    return np.asarray(rotated)


# ---------------------------------------------------------------- 动作帧


def breathe_frames(img: Image.Image, template: dict, n: int) -> list[np.ndarray]:
    p_alpha = alpha_bounds(img)
    t_alpha = tuple(template["space"]["alphaBounds"].values())
    zone = map_rect(template["space"]["breathZone"], t_alpha, p_alpha)
    amp = template["amplitude"]["breath"]
    base = np.asarray(img.convert("RGBA"))
    frames = []
    for i in range(n):
        phase = 2 * math.pi * i / n
        scale = 1.0 + float(amp["max"]) * math.sin(phase)
        frames.append(warp_region(base, zone, lambda r, s=scale: _scale_vertical(r, s, "top")))
    return frames


def blink_frames(img: Image.Image, template: dict, n: int, blinks: int = 2) -> list[np.ndarray]:
    p_alpha = alpha_bounds(img)
    t_alpha = tuple(template["space"]["alphaBounds"].values())
    base = np.asarray(img.convert("RGBA"))
    eyes = template["space"]["eyes"]
    left = map_rect(eyes["left"]["bounds"], t_alpha, p_alpha)
    right = map_rect(eyes["right"]["bounds"], t_alpha, p_alpha)
    frames = []
    for i in range(n):
        t = i / max(1, n - 1)
        # blinks 次眨眼，每次：睁开 -> 闭合 -> 睁开
        cycle = (t * blinks) % 1.0
        close = 1.0 if cycle < 0.25 else (0.0 if cycle > 0.75 else 1.0 - 4 * (cycle - 0.25) if cycle < 0.5 else 4 * (cycle - 0.75))
        close = min(1.0, max(0.0, close))
        ratio = 1.0 - 0.9 * close  # 1 全开 -> 0.1 全闭
        frame = warp_region(base, left, lambda r, q=ratio: _squash(r, q))
        frame = warp_region(frame, right, lambda r, q=ratio: _squash(r, q))
        frames.append(frame)
    return frames


def tail_frames(img: Image.Image, template: dict, n: int) -> list[np.ndarray]:
    p_alpha = alpha_bounds(img)
    t_alpha = tuple(template["space"]["alphaBounds"].values())
    tail = next(p for p in template["parts"] if p["id"] == "tail")
    bounds = map_rect(tail["bounds"], t_alpha, p_alpha)
    root = map_point(template["space"]["tailRoot"], t_alpha, p_alpha)
    # pivot 用 tailRoot 相对 tail bounds 的比例
    px = (root[0] - bounds[0]) / max(1, (bounds[2] - bounds[0]))
    py = (root[1] - bounds[1]) / max(1, (bounds[3] - bounds[1]))
    pivot = (max(0.0, min(1.0, px)), max(0.0, min(1.0, py)))
    angle = float(template["amplitude"]["tailAngle"]["max"]) * 90  # 弧度->角度系
    base = np.asarray(img.convert("RGBA"))
    frames = []
    for i in range(n):
        phase = 2 * math.pi * i / n
        deg = angle * math.sin(phase)
        frames.append(warp_region(base, bounds, lambda r, d=deg: _rotate(r, d, pivot)))
    return frames


# ---------------------------------------------------------------- 输出


def save_gif(frames: list[np.ndarray], out_path: Path, duration_ms: int = DEFAULT_DURATION_MS) -> None:
    images = []
    for frame in frames:
        im = Image.fromarray(frame).convert("RGBA")
        if im.width > PREVIEW_SIZE or im.height > PREVIEW_SIZE:
            im = im.resize((PREVIEW_SIZE, PREVIEW_SIZE), Image.NEAREST)
        images.append(im.convert("P", palette=Image.Palette.ADAPTIVE, colors=256))
    images[0].save(
        out_path,
        save_all=True,
        append_images=images[1:],
        duration=duration_ms,
        loop=0,
        disposal=2,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate pixel pet motion demo GIFs (M2)")
    parser.add_argument("body_png", type=Path, help="pixel pet body.png path")
    parser.add_argument("-o", "--output", type=Path, default=SCRIPT_DIR.parent / "output" / "pixel-motion-demo")
    parser.add_argument("--frames", type=int, default=DEFAULT_FRAMES)
    args = parser.parse_args()

    if not args.body_png.is_file():
        print(f"body.png not found: {args.body_png}", file=sys.stderr)
        return 2
    template = load_template()
    img = Image.open(args.body_png)

    args.output.mkdir(parents=True, exist_ok=True)
    results = {
        "breath.gif": breathe_frames(img, template, args.frames),
        "blink.gif": blink_frames(img, template, args.frames),
        "tail.gif": tail_frames(img, template, args.frames),
    }
    for name, frames in results.items():
        out = args.output / name
        save_gif(frames, out)
        print(f"wrote {out} ({len(frames)} frames)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

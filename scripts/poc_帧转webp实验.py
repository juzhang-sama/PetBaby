# -*- coding: utf-8 -*-
"""WebP 帧压缩实验：量化各档位收益 + 生成视觉对比证据图。

对 04/05 抽样帧做 PNG → WebP（lossless / q95 / q90 / q85），输出：
1. 体积表（原 PNG vs 各档 WebP）
2. 证据图：棋盘格底合成 + alpha 边缘放大 + RGB 差异热图（判断有损是否可见）
"""
from __future__ import annotations

import io
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
PETS = {
    "04": ROOT / "apps/desktop/public/builtin-pets/04-warm-brown-tabby",
    "05": ROOT / "apps/desktop/public/builtin-pets/05-silver-tabby",
}

QUALITY_LEVELS = [
    ("lossless", dict(lossless=True, method=6)),
    ("q95", dict(lossless=False, quality=95, method=6)),
    ("q90", dict(lossless=False, quality=90, method=6)),
    ("q85", dict(lossless=False, quality=85, method=6)),
]


def sample_frames(pet_dir: Path, per_action: int = 2) -> list[tuple[str, Path]]:
    out = []
    for action_dir in sorted((pet_dir / "frames").iterdir()):
        frames = sorted(action_dir.glob("*.png"))
        if not frames:
            continue
        # 均匀抽样
        idx = np.linspace(0, len(frames) - 1, per_action).astype(int)
        for i in idx:
            out.append((f"{action_dir.name}/{frames[i].name}", frames[i]))
    return out


def encode_webp(img: Image.Image, params: dict) -> bytes:
    buf = io.BytesIO()
    img.save(buf, "WEBP", **params)
    return buf.getvalue()


def checker(size: int, cell: int = 16) -> Image.Image:
    arr = np.zeros((size, size, 4), np.uint8)
    arr[:, :, 3] = 255
    y, x = np.mgrid[0:size, 0:size]
    c = ((y // cell) + (x // cell)) % 2
    arr[:, :, :3] = np.where(c[:, :, None] == 1, 255, 40)
    return Image.fromarray(arr, "RGBA")


def composite(fg: Image.Image, bg: Image.Image) -> Image.Image:
    bg = bg.resize(fg.size, Image.NEAREST)
    return Image.alpha_composite(bg.convert("RGBA"), fg.convert("RGBA"))


def alpha_edge_tile(img: Image.Image, box: tuple[int, int, int, int], scale: int = 4) -> Image.Image:
    crop = img.crop(box).resize(
        ((box[2] - box[0]) * scale, (box[3] - box[1]) * scale), Image.NEAREST
    )
    return crop


def rgb_diff(a: Image.Image, b: Image.Image) -> Image.Image:
    aa = np.asarray(a.convert("RGBA")).astype(np.int16)
    bb = np.asarray(b.convert("RGBA")).astype(np.int16)
    d = np.abs(aa[:, :, :3] - bb[:, :, :3]).max(axis=2)
    d = np.clip(d * 8, 0, 255).astype(np.uint8)  # 放大 8 倍便于肉眼
    # 透明区差异清零
    d[(aa[:, :, 3] < 128) & (bb[:, :, 3] < 128)] = 0
    return Image.fromarray(d, "L").convert("RGBA")


def find_alpha_edge_bbox(img: Image.Image) -> tuple[int, int, int, int]:
    """找 alpha 过渡带（128±64）的 bbox，放大看边缘质量。"""
    a = np.asarray(img.convert("RGBA"))[:, :, 3]
    edge = (a > 64) & (a < 192)
    ys, xs = np.nonzero(edge)
    if len(xs) == 0:
        # 退化：取整图中心
        w, h = img.size
        return (w // 4, h // 4, 3 * w // 4, 3 * h // 4)
    pad = 12
    x0 = max(0, xs.min() - pad); x1 = min(img.size[0], xs.max() + pad)
    y0 = max(0, ys.min() - pad); y1 = min(img.size[1], ys.max() + pad)
    return (x0, y0, x1, y1)


def main() -> int:
    out_root = ROOT / "output" / "webp压缩实验-2026-09-04"
    out_root.mkdir(parents=True, exist_ok=True)

    print("帧 | 原PNG |", " | ".join(f"{name}(KB)" for name, _ in QUALITY_LEVELS))
    print("-" * 100)

    for tag, pet_dir in PETS.items():
        samples = sample_frames(pet_dir)
        bg = checker(600)
        # 证据拼图：每个抽样帧一张，行=帧，列=[原图, lossless, q95, q90, q85]
        rows = []
        for name, path in samples:
            src = Image.open(path).convert("RGBA")
            src_bytes = path.stat().st_size
            cells = [composite(src, bg)]
            sizes = []
            for _, params in QUALITY_LEVELS:
                webp = encode_webp(src, params)
                sizes.append(len(webp))
                dec = Image.open(io.BytesIO(webp)).convert("RGBA")
                cells.append(composite(dec, bg))
            row = " | ".join(
                f"{name}({src_bytes//1024}K)" + " " + " ".join(f"{s//1024}K" for s in sizes)
            )
            print(f"{tag}/{row}")
            # 拼一行缩略图
            thumb_w = 120
            thumb = [c.resize((thumb_w, thumb_w), Image.LANCZOS) for c in cells]
            line = Image.new("RGBA", (thumb_w * len(thumb) + 10 * (len(thumb) - 1), thumb_w), (0, 0, 0, 255))
            x = 0
            for t in thumb:
                line.paste(t, (x, 0)); x += thumb_w + 10
            rows.append(line)

        grid = Image.new("RGBA", (rows[0].width, sum(r.height for r in rows) + 6 * (len(rows) - 1)), (20, 20, 20, 255))
        y = 0
        for r in rows:
            grid.paste(r, (0, y)); y += r.height + 6
        grid.convert("RGB").save(out_root / f"{tag}-棋盘格对比.png")

        # 边缘放大证据：取一帧（最后一个动作的一帧）的边缘过渡带
        src = Image.open(samples[len(samples) // 2][1]).convert("RGBA")
        bbox = find_alpha_edge_bbox(src)
        edge_cells = []
        for _, params in QUALITY_LEVELS:
            dec = Image.open(io.BytesIO(encode_webp(src, params))).convert("RGBA")
            edge_cells.append(alpha_edge_tile(composite(dec, bg), bbox))
        ew = edge_cells[0].width
        edge_line = Image.new("RGBA", (ew * len(edge_cells) + 8 * (len(edge_cells) - 1), edge_cells[0].height), (0, 0, 0, 255))
        x = 0
        for c in edge_cells:
            edge_line.paste(c, (x, 0)); x += ew + 8
        edge_line.convert("RGB").save(out_root / f"{tag}-alpha边缘放大.png")

        # RGB 差异热图（有损档 vs 原图）
        diff_cells = []
        for _, params in QUALITY_LEVELS:
            dec = Image.open(io.BytesIO(encode_webp(src, params))).convert("RGBA")
            diff_cells.append(rgb_diff(src, dec))
        dw = 300
        diff_line = Image.new("RGBA", (dw * len(diff_cells) + 8 * (len(diff_cells) - 1), dw), (0, 0, 0, 255))
        x = 0
        for d in diff_cells:
            diff_line.paste(d.resize((dw, dw)), (x, 0)); x += dw + 8
        diff_line.convert("RGB").save(out_root / f"{tag}-RGB差异热图.png")

    print("\n证据图输出到", out_root)
    return 0


if __name__ == "__main__":
    sys.exit(main())

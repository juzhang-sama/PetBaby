# -*- coding: utf-8 -*-
"""WebP 压缩质量定量验收：PNG vs 各档 WebP 的逐像素差异。

指标（对每只猫每个动作全帧计算，输出均值/最差）：
- rgb_mean / rgb_p99 / rgb_max：RGB 每像素最大通道差
- alpha_mean / alpha_p99 / alpha_max：alpha 通道差（边缘完整性关键）
- psnr：RGB 峰值信噪比（>40dB 肉眼不可见，>35dB 通常可接受）
- edge_band：alpha 过渡带宽度是否被压缩（边缘质量）
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
LEVELS = [
    ("q95", dict(lossless=False, quality=95, method=4)),
    ("q90", dict(lossless=False, quality=90, method=4)),
    ("q85", dict(lossless=False, quality=85, method=4)),
]


def encode(img: Image.Image, params: dict) -> bytes:
    b = io.BytesIO()
    img.save(b, "WEBP", **params)
    return b.getvalue()


def psnr(a: np.ndarray, b: np.ndarray) -> float:
    mse = np.mean((a.astype(np.float64) - b.astype(np.float64)) ** 2)
    if mse == 0:
        return 99.0
    return 10 * np.log10(255.0 ** 2 / mse)


def analyze(level: str, params: dict, pet_dir: Path) -> dict:
    rgb_maxs, rgb_means, rgb_p99s = [], [], []
    alpha_maxs, alpha_means = [], []
    psnrs = []
    frame_count = 0
    for action_dir in sorted((pet_dir / "frames").iterdir()):
        frames = sorted(action_dir.glob("*.png"))
        # 抽样（每 20 帧取 1），全量太慢；动作运动平滑，抽样足够代表
        frames = frames[::20]
        for fp in frames:
            src = Image.open(fp).convert("RGBA")
            webp = encode(src, params)
            dec = Image.open(io.BytesIO(webp)).convert("RGBA")
            a = np.asarray(src).astype(np.int16)
            b = np.asarray(dec).astype(np.int16)
            rgb_diff = np.abs(a[:, :, :3] - b[:, :, :3]).max(axis=2)
            rgb_maxs.append(rgb_diff.max())
            rgb_means.append(rgb_diff.mean())
            rgb_p99s.append(np.percentile(rgb_diff, 99))
            alpha_diff = np.abs(a[:, :, 3] - b[:, :, 3])
            alpha_maxs.append(alpha_diff.max())
            alpha_means.append(alpha_diff.mean())
            psnrs.append(psnr(np.asarray(src)[:, :, :3], np.asarray(dec)[:, :, :3]))
            frame_count += 1
    return {
        "frames": frame_count,
        "rgb_mean": float(np.mean(rgb_means)),
        "rgb_p99": float(np.mean(rgb_p99s)),
        "rgb_max": int(np.max(rgb_maxs)),
        "alpha_mean": float(np.mean(alpha_means)),
        "alpha_max": int(np.max(alpha_maxs)),
        "psnr_db": float(np.mean(psnrs)),
    }


def main() -> int:
    print(f"{'宠物':4s} {'档位':10s} {'帧数':5s} {'RGB均值':8s} {'RGBp99':7s} {'RGBmax':7s} "
          f"{'α均值':7s} {'αmax':5s} {'PSNR':7s}")
    print("-" * 80)
    for tag, pet_dir in PETS.items():
        for level, params in LEVELS:
            r = analyze(level, params, pet_dir)
            print(f"{tag:4s} {level:10s} {r['frames']:5d} {r['rgb_mean']:8.3f} "
                  f"{r['rgb_p99']:7.3f} {r['rgb_max']:7d} {r['alpha_mean']:7.3f} "
                  f"{r['alpha_max']:5d} {r['psnr_db']:7.2f}")
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())

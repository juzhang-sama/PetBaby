# -*- coding: utf-8 -*-
"""拎起母版 vs 坐姿母版：量化画风差异 + identity 一致性。

老王反馈新图"画风与当前形象画风不太一样"，本脚本量化差异，定位具体是哪一块不对：
- 背景：是否透明（坐姿母版是 RGBA 透明底）
- 姿态：主体 bbox 长宽比（拎起应竖长，坐姿偏方/略竖）
- 毛色：主体 RGB 均值 + HSV 饱和/色相（identity 一致性）
- 画风：描边强度（坐姿母版有 dark contour line）、颜色量化程度（flat cel shading = 色块少）
"""
from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(r"D:\petBaby\desktop-pet")
SEATED = {
    "04": ROOT / "output/宠物动作-毛砌墙-v3-2026-08-31/00-母版/母版-121107408.png",
    "05": ROOT / "output/宠物动作-建国-v1-2026-09-03/00-母版/母版-124423137.png",
}
LIFTED = {
    "04": Path(r"C:\Users\Administrator\Downloads\gpt-image-2-auto.png"),
    "05": Path(r"C:\Users\Administrator\Downloads\gpt-image-2-auto (1).png"),
}


def load_rgba(p: Path) -> np.ndarray:
    im = Image.open(p)
    if im.mode != "RGBA":
        im = im.convert("RGBA")
    return np.asarray(im)


def analyze(name: str, path: Path) -> dict:
    a = load_rgba(path)
    h, w = a.shape[:2]
    alpha = a[:, :, 3]
    body = alpha > 200
    transparent_ratio = float((alpha < 32).mean())

    if body.sum() < 1000:
        return {"name": name, "w": w, "h": h, "error": "主体太小或无透明背景"}

    ys, xs = np.nonzero(body)
    bbox = (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max()))
    bw = xs.max() - xs.min()
    bh = ys.max() - ys.min()

    rgb = a[:, :, :3]
    body_rgb = rgb[body]
    hsv = np.asarray(Image.fromarray(rgb[body].reshape(-1, 1, 3), "RGB").convert("HSV")).reshape(-1, 3)

    # 颜色量化程度：对主体 RGB 做 4-bit 量化后统计唯一颜色数（flat cel = 色块少）
    quant = (body_rgb // 32).astype(np.int32)
    unique_colors = len(np.unique(quant, axis=0))

    # 描边强度：主体边缘（alpha 过渡带）的亮度 vs 主体内部亮度（描边 = 边缘更暗）
    edge_band = (alpha >= 128) & (alpha < 250)
    interior = alpha >= 250
    edge_lum = float(rgb[edge_band].mean()) if edge_band.sum() > 100 else 0.0
    interior_lum = float(rgb[interior].mean()) if interior.sum() > 100 else 0.0
    contour_strength = interior_lum - edge_lum  # 正值 = 边缘比内部暗（有描边）

    return {
        "name": name,
        "size": f"{w}x{h}",
        "transparent_ratio": round(transparent_ratio * 100, 1),
        "bbox": bbox,
        "aspect_wh": round(bh / max(1, bw), 2),  # 高/宽比
        "rgb_mean": [round(float(v), 1) for v in body_rgb.mean(0)],
        "sat_median": round(float(np.median(hsv[:, 1])), 0),
        "hue_median": round(float(np.median(hsv[:, 0])), 0),
        "unique_colors": unique_colors,
        "contour_strength": round(contour_strength, 1),
    }


def main() -> int:
    print("=== 坐姿母版（参照，画风基准）===")
    seated = {}
    for tag, p in SEATED.items():
        r = analyze(f"{tag}-坐姿", p)
        seated[tag] = r
        print(f"  {r}")
    print("\n=== 拎起新图 ===")
    for tag, p in LIFTED.items():
        r = analyze(f"{tag}-拎起", p)
        print(f"  {r}")
        s = seated.get(tag)
        if s and "error" not in r and "error" not in s:
            print(f"    对比坐姿: 长宽比 {s['aspect_wh']}→{r['aspect_wh']}  "
                  f"RGB距离={np.linalg.norm(np.array(r['rgb_mean'])-np.array(s['rgb_mean'])):.1f}  "
                  f"色相 {s['hue_median']}→{r['hue_median']}  饱和 {s['sat_median']}→{r['sat_median']}  "
                  f"色块 {s['unique_colors']}→{r['unique_colors']}  描边 {s['contour_strength']}→{r['contour_strength']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

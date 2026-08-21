"""分析成品像素猫的皮肤数据：毛色（k-means）、花纹 patch、眼色。

输出到 output/pixel-motion-demo/v3/skin-data.json，供合成脚本使用。
工作层：512 逻辑像素网格（2048 成品图 = 512 网格 × 4 放大）。
"""
import json
import sys
from collections import Counter

import numpy as np
from PIL import Image

LOGICAL = 512  # 逻辑像素网格尺寸


def to_logical(img: Image.Image) -> np.ndarray:
    """把 2048 成品图降采样到 512 逻辑网格（4x4 块平均 = 逻辑像素色）。"""
    if img.width != LOGICAL:
        small = img.resize((LOGICAL, LOGICAL), Image.BOX)
    else:
        small = img
    return np.asarray(small.convert("RGBA")).astype(np.int16)


def kmeans_colors(pixels: np.ndarray, k: int, seed: int = 7) -> list[tuple[tuple[int, int, int], float]]:
    """手写 k-means，返回 [(rgb, 占比)]。pixels: Nx3 int16。"""
    rng = np.random.default_rng(seed)
    idx = rng.choice(len(pixels), size=min(k, len(pixels)), replace=False)
    centers = pixels[idx].astype(np.float64)
    for _ in range(30):
        dist = ((pixels[:, None, :] - centers[None, :, :]) ** 2).sum(axis=2)
        assign = dist.argmin(axis=1)
        new_centers = np.array(
            [pixels[assign == c].mean(axis=0) if np.any(assign == c) else centers[c] for c in range(k)]
        )
        if np.abs(new_centers - centers).max() < 0.5:
            centers = new_centers
            break
        centers = new_centers
    counts = Counter(assign.tolist())
    total = len(pixels)
    out = []
    for c in range(k):
        rgb = tuple(int(round(v)) for v in centers[c])
        out.append((rgb, counts.get(c, 0) / total))
    out.sort(key=lambda x: -x[1])
    return out


def main(path: str) -> None:
    img = Image.open(path).convert("RGBA")
    a = to_logical(img)
    alpha = a[:, :, 3]
    ys, xs = np.where(alpha > 0)
    if len(xs) == 0:
        print("!! 无主体")
        sys.exit(1)
    alpha_bbox = (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max()))
    print(f"alphaBounds(512网格): L={alpha_bbox[0]} T={alpha_bbox[1]} R={alpha_bbox[2]} B={alpha_bbox[3]}")
    print(f"主体尺寸: {alpha_bbox[2]-alpha_bbox[0]}x{alpha_bbox[3]-alpha_bbox[1]}")

    # 主体像素（忽略半透明/边缘）
    mask = alpha > 128
    rgb = a[:, :, :3][mask]

    # 1) 毛色 k-means
    colors = kmeans_colors(rgb, 7)
    print("\n=== 毛色聚类（按占比） ===")
    for rgb_c, frac in colors:
        print(f"  #{rgb_c[0]:02X}{rgb_c[1]:02X}{rgb_c[2]:02X}  rgb{rgb_c}  占比 {frac:.1%}")

    # 2) 描边 = 最暗的簇（像素风深色轮廓）；主色 = 排除描边后占比最高的
    dark = [c for c in colors if sum(c[0]) < 200]
    outline_c = dark[0] if dark else colors[-1]
    outline_rgb = np.array(outline_c[0])
    fur = [c for c in colors if c[0] != outline_c[0]]
    main_c = fur[0] if fur else colors[0]
    main_rgb = np.array(main_c[0])
    print(f"\n描边色(最暗): {outline_rgb.tolist()} (占比 {outline_c[1]:.1%})")
    print(f"主毛色: {main_rgb.tolist()} (占比 {main_c[1]:.1%})")

    # 3) 花纹 = 与主毛色显著不同、且非描边的像素（副色区域）
    dist_main = np.abs(rgb.astype(int) - main_rgb).sum(axis=1)
    dist_dark = np.abs(rgb.astype(int) - outline_rgb).sum(axis=1)
    pattern_pixels = rgb[(dist_main > 120) & (dist_dark > 150)]
    print(f"花纹候选像素数: {len(pattern_pixels)} / {len(rgb)}")
    patch_rgba = None
    if len(pattern_pixels) > 800:
        pcolors = kmeans_colors(pattern_pixels, 3)
        print("=== 花纹色 ===")
        for rgb_c, frac in pcolors:
            print(f"  #{rgb_c[0]:02X}{rgb_c[1]:02X}{rgb_c[2]:02X}  {frac:.1%}")
        # 裁花纹 patch：花纹像素质心区域（含一点毛色作过渡）
        ys_all, xs_all = np.where(mask)
        # 需要逐像素判断花纹
        pmask_flat = (dist_main > 120) & (dist_dark > 150)
        px, py = xs_all[pmask_flat], ys_all[pmask_flat]
        if len(px) > 50:
            cx, cy = int(np.median(px)), int(np.median(py))
            w = min(max(int((px.max() - px.min()) * 1.3), 48), 112)
            h = min(max(int((py.max() - py.min()) * 1.3), 48), 112)
            patch = a[
                max(0, cy - h // 2): min(LOGICAL, cy + h // 2),
                max(0, cx - w // 2): min(LOGICAL, cx + w // 2),
                :,
            ]
            patch_alpha = patch[:, :, 3]
            if patch_alpha.size > 0 and (patch_alpha > 128).mean() > 0.1:
                print(f"花纹 patch: 中心({cx},{cy}) 尺寸 {patch.shape[1]}x{patch.shape[0]} 非透明占比 {(patch_alpha>128).mean():.1%}")
                patch_rgba = patch.astype(np.uint8)
                Image.fromarray(patch_rgba).save("output/pixel-motion-demo/v3/pattern-patch.png")
                print("已保存 output/pixel-motion-demo/v3/pattern-patch.png")
            else:
                print("花纹 patch 区域太稀疏，跳过")
        else:
            print("花纹像素太少，视为纯色")
    else:
        print("无显著花纹（纯色猫）")

    # 3) 眼色：在眼睛区域（模板 eyes bounds 映射到 512）找最暗的非描边像素
    # 眼睛区域近似：主体上部 1/3、水平中部 40%
    x0, y0, x1, y1 = alpha_bbox
    eye_zone = a[
        y0 + int((y1 - y0) * 0.18): y0 + int((y1 - y0) * 0.38),
        x0 + int((x1 - x0) * 0.3): x0 + int((x1 - x0) * 0.7),
        :,
    ]
    ez_mask = eye_zone[:, :, 3] > 128
    ez_rgb = eye_zone[:, :, :3][ez_mask]
    if len(ez_rgb) > 50:
        # 眼睛 = 区域内最暗的 2% 像素的平均
        lum = ez_rgb.sum(axis=1)
        eye_px = ez_rgb[lum < np.percentile(lum, 5)]
        if len(eye_px) > 3:
            eye_color = tuple(int(round(v)) for v in eye_px.mean(axis=0))
        else:
            eye_color = tuple(int(v) for v in dark_rgb)
        print(f"\n眼色(眼部最暗5%): {eye_color}")
    else:
        eye_color = tuple(int(v) for v in dark_rgb)
        print(f"\n眼色: 默认 {eye_color}")

    # 4) 保存
    out = {
        "source": path,
        "logical": LOGICAL,
        "alphaBounds": alpha_bbox,
        "mainColor": tuple(int(v) for v in main_rgb),
        "outlineColor": tuple(int(v) for v in outline_rgb),
        "eyeColor": eye_color,
        "colors": [{"rgb": c[0], "share": round(c[1], 4)} for c in colors],
        "hasPattern": patch_rgba is not None,
    }
    os.makedirs("output/pixel-motion-demo/v3", exist_ok=True)
    with open("output/pixel-motion-demo/v3/skin-data.json", "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=2)
    print("\n已保存 output/pixel-motion-demo/v3/skin-data.json")


if __name__ == "__main__":
    import os

    main(sys.argv[1])

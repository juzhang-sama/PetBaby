# -*- coding: utf-8 -*-
"""拎起母版取景兼容性体检：拎起姿态能否装进 idle 的 crop box，装进去多大。

核心问题：拎起是竖长条（四腿收拢），坐姿近方形。复用 idle crop box 时，
拎起猫会变窄，这里量化变窄多少 + 生成对比图给老王肉眼判断。
"""
from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(r"D:\petBaby\desktop-pet")
SEATED = ROOT / "output/宠物动作-毛砌墙-v3-2026-08-31/00-母版/母版-121107408.png"
LIFTED = ROOT / "output/宠物动作-毛砌墙-v3-2026-08-31/12-拎起/00-拎起母版/拎起母版-暖棕.png"
OUT = ROOT / "output/宠物动作-毛砌墙-v3-2026-08-31/12-拎起/00-拎起母版"

# 04 idle crop（复用 idle-combo 的取景框）：源 640 空间 crop(9,25,596)，输出 588
CROP = {"x": 9, "y": 25, "size": 596, "source": 640, "output": 588}


def bbox_of(a: np.ndarray, thr: int = 200) -> tuple[int, int, int, int]:
    ys, xs = np.nonzero(a[:, :, 3] > thr)
    return int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())


def main() -> int:
    seated = np.asarray(Image.open(SEATED).convert("RGBA"))
    lifted = np.asarray(Image.open(LIFTED).convert("RGBA"))

    sb = bbox_of(seated)
    lb = bbox_of(lifted)
    sw, sh = sb[2] - sb[0], sb[3] - sb[1]
    lw, lh = lb[2] - lb[0], lb[3] - lb[1]

    print(f"坐姿母版: bbox={sb}  宽{sw} 高{sh}  长宽比 {sh/sw:.2f}")
    print(f"拎起母版: bbox={lb}  宽{lw} 高{lh}  长宽比 {lh/lw:.2f}")

    # 缩放到 idle crop box：高度方向填满 crop（两者都是高>宽）
    scale = CROP["size"] / max(sh, lh)
    seated_w = sw * scale
    lifted_w = lw * scale
    print(f"\n按高度填满 crop({CROP['size']}px)缩放，实际显示宽度：")
    print(f"  坐姿宽 = {seated_w:.0f}px（占 {seated_w/CROP['size']*100:.1f}%）")
    print(f"  拎起宽 = {lifted_w:.0f}px（占 {lifted_w/CROP['size']*100:.1f}%）")
    print(f"  拎起宽度是坐姿的 {lifted_w/seated_w*100:.0f}%")

    # 生成对比图：两只猫缩放到相同 crop box，并排
    def to_crop(a: np.ndarray) -> Image.Image:
        im = Image.fromarray(a)
        im = im.resize((CROP["output"], CROP["output"]), Image.LANCZOS)
        return im

    checker = Image.new("RGBA", (CROP["output"], CROP["output"]), (60, 60, 60, 255))
    # 简单棋盘格
    arr = np.asarray(checker).copy()
    cell = 40
    y, x = np.mgrid[0:CROP["output"], 0:CROP["output"]]
    arr[:, :, :3] = np.where((((y // cell) + (x // cell)) % 2) == 1, 220, 40)[:, :, None]
    checker = Image.fromarray(arr, "RGBA")

    comp_seated = Image.alpha_composite(checker, to_crop(seated))
    comp_lifted = Image.alpha_composite(checker, to_crop(lifted))
    gap = Image.new("RGBA", (20, CROP["output"]), (0, 0, 0, 255))
    side = Image.new("RGBA", (CROP["output"] * 2 + 20, CROP["output"]), (0, 0, 0, 255))
    side.paste(comp_seated, (0, 0))
    side.paste(gap, (CROP["output"], 0))
    side.paste(comp_lifted, (CROP["output"] + 20, 0))
    side.convert("RGB").save(OUT / "取景对比-坐姿vs拎起.png")

    print(f"\n对比图输出: {OUT / '取景对比-坐姿vs拎起.png'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

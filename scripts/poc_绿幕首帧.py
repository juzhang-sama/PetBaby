# -*- coding: utf-8 -*-
"""把透明母版合成到纯绿幕上，生成 Seedance 图生视频首帧（薄 CLI）。

**逻辑已搬进服务**：`photo_avatar_backend.frames.first_frame.compose_green_first_frame`。
本文件只做参数解析、候选母版清单与打印，避免脚本和服务各维护一份实现。

用法：
    D:/DevTools/Python312/python.exe scripts/poc_绿幕首帧.py
    D:/DevTools/Python312/python.exe scripts/poc_绿幕首帧.py --master <母版.png> --scale 0.90 --margin-left 0.06 --outdir <目录>

输出：
    <outdir>/绿幕首帧-1024.jpg   （默认首帧）
    <outdir>/绿幕首帧-1024.png   （无损备选）
    <outdir>/母版-1024.png       （硬化并取景后的透明母版）
    <outdir>/首帧分析.json

关键约定：
  - 绿幕必须是纯 (0,255,0)，全程无渐变、无纹理、无阴影；
  - 主体 alpha 硬化到 255，避免母版残留的 252 造成整体偏绿；
  - 边缘保留原始抗锯齿过渡，因为这正是抠像要解决的难点；
  - **`tailSwingFit` 里两个余量都要 ≥5%**，否则尾巴会出画、视频白花钱。
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICE_SRC = ROOT / "services" / "appearance-generation" / "src"
sys.path.insert(0, str(SERVICE_SRC))

from photo_avatar_backend.frames.first_frame import (  # noqa: E402
    MIN_FRAMING_MARGIN,
    FirstFrameError,
    compose_green_first_frame,
)

OUT_DIR = ROOT / "output" / "POC-绿幕视频-2026-08-29" / "01-首帧"

# 候选母版：优先用边缘最干净的强化版（不指定 --master 时用）
CANDIDATES = [
    ("01-中度卡通2D-强化版", ROOT / "output" / "新风格对比-2026-08-28" / "01-中度卡通2D-强化版" / "母版.png"),
    ("01-中度卡通2D", ROOT / "output" / "新风格对比-2026-08-28" / "01-中度卡通2D" / "母版.png"),
    ("02-半写实贴纸", ROOT / "output" / "新风格对比-2026-08-28" / "02-半写实贴纸" / "母版.png"),
    ("03-纯写实照片分身", ROOT / "output" / "新风格对比-2026-08-28" / "03-纯写实照片分身" / "母版.png"),
]


def main() -> int:
    parser = argparse.ArgumentParser(description="生成绿幕首帧")
    parser.add_argument("--scale", type=float, default=0.9,
                        help="母版缩放，留出尾巴甩动空间；1.0 表示不缩（1:1 画幅下会切尾巴）")
    parser.add_argument("--margin-left", type=float, default=0.05,
                        help="缩放后猫的左边界位置（占画布比例），越小右侧留给尾巴的空间越多")
    parser.add_argument("--outdir", type=str, default=None, help="输出目录")
    parser.add_argument("--master", type=str, default=None,
                        help="直接指定母版 PNG；不指定则从 CANDIDATES 里自动挑边缘最干净的")
    args = parser.parse_args()

    out_dir = Path(args.outdir).resolve() if args.outdir else OUT_DIR

    if args.master:
        chosen = Path(args.master).resolve()
        masters = [(chosen.stem, chosen)]
    else:
        masters = CANDIDATES

    try:
        result = compose_green_first_frame(
            masters,
            out_dir,
            scale=args.scale,
            margin_left=args.margin_left,
            path_base=ROOT,
        )
    except FirstFrameError as exc:
        print(f"[FAIL] {exc}")
        return 1

    fit = result.tail_swing_fit
    if fit is None:
        print(f"[警告] scale=1.0 未做取景重排，无法判断取景余量，"
              f"应改用小于 1 的 scale（下限 {MIN_FRAMING_MARGIN * 100:.0f}%）")
    elif not result.framing_ok:
        print(f"[警告] 取景余量不足 {MIN_FRAMING_MARGIN * 100:.0f}%"
              f"（左 {result.left_margin * 100:.1f}% / 右 {result.right_margin_at_full_swing * 100:.1f}%）"
              f"—— **不许进视频**，请调小 --scale 重生成")

    print(f"[分析] {result.analysis_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

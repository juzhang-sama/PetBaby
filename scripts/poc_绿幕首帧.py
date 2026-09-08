# -*- coding: utf-8 -*-
"""POC：把透明母版合成到纯绿幕上，生成 Seedance 图生视频首帧。

这一步不接入产品，只为验证：
  绿幕视频 -> 自动抠像 -> 透明 PNG 帧序列 -> frame-sequence 预览

用法：
  D:/DevTools/Python312/python.exe scripts/poc_绿幕首帧.py

输出：
  output/POC-绿幕视频-2026-08-29/01-首帧/绿幕首帧-1024.jpg
  output/POC-绿幕视频-2026-08-29/01-首帧/母版-1024.png   （硬化后的透明母版，供对比）
  output/POC-绿幕视频-2026-08-29/01-首帧/首帧分析.json

关键约定：
  - 绿幕必须是纯 (0,255,0)，全程无渐变、无纹理、无阴影；
  - 主体 alpha 硬化到 255，避免母版残留的 252 造成整体偏绿；
  - 边缘保留原始抗锯齿过渡，因为这正是抠像要解决的难点。
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
OUT_DIR = ROOT / "output" / "POC-绿幕视频-2026-08-29" / "01-首帧"

# 候选母版：优先用边缘最干净的强化版
CANDIDATES = [
    ("01-中度卡通2D-强化版", ROOT / "output" / "新风格对比-2026-08-28" / "01-中度卡通2D-强化版" / "母版.png"),
    ("01-中度卡通2D", ROOT / "output" / "新风格对比-2026-08-28" / "01-中度卡通2D" / "母版.png"),
    ("02-半写实贴纸", ROOT / "output" / "新风格对比-2026-08-28" / "02-半写实贴纸" / "母版.png"),
    ("03-纯写实照片分身", ROOT / "output" / "新风格对比-2026-08-28" / "03-纯写实照片分身" / "母版.png"),
]

SIZE = 1024
GREEN = (0, 255, 0)
ALPHA_HARDEN = 240      # alpha >= 240 视为主体，硬化为 255
ALPHA_KILL = 16         # alpha <= 16 视为背景噪声，清零

# 摇尾时尾尖摆幅 / 猫身宽度。由 1:1 那支实测：右边界在 795~959 间摆动，
# 摆幅 164px，单帧猫宽 699px -> 0.235。换宠物或换动作幅度要重新测。
TAIL_SWING_RATIO = 0.235


def analyze(mask: np.ndarray) -> dict:
    """用形态学腐蚀估算细结构（胡须、毛尖）占比。"""
    stats = {
        "foregroundRatio": round(float(mask.mean()), 4),
    }
    ys, xs = np.nonzero(mask)
    if ys.size:
        stats["bbox"] = {
            "x": int(xs.min()), "y": int(ys.min()),
            "w": int(xs.max() - xs.min() + 1), "h": int(ys.max() - ys.min() + 1),
        }
        stats["bboxFillRatio"] = round(float(mask[ys.min():ys.max() + 1, xs.min():xs.max() + 1].mean()), 4)
    else:
        stats["bbox"] = None
        stats["bboxFillRatio"] = 0.0

    try:
        import cv2
        u8 = (mask * 255).astype(np.uint8)
        kernel = np.ones((3, 3), np.uint8)
        e1 = cv2.erode(u8, kernel, iterations=1) > 0
        e2 = cv2.erode(u8, kernel, iterations=2) > 0
        area = float(mask.sum())
        # 细结构 = 腐蚀一次就消失的部分（宽度 <=2px 的胡须、毛尖）
        stats["thinStructureRatio"] = round(float((mask.sum() - e1.sum()) / area), 4) if area else 0.0
        stats["veryThinStructureRatio"] = round(float((mask.sum() - e2.sum()) / area), 4) if area else 0.0
    except ImportError:
        stats["thinStructureRatio"] = None
        stats["veryThinStructureRatio"] = None
    return stats


def prepare_master(src: Path) -> tuple[Image.Image, dict]:
    """读取母版 -> 硬化 alpha -> 缩放到 1024。"""
    im = Image.open(src).convert("RGBA")
    arr = np.array(im).astype(np.float32)
    a = arr[:, :, 3]

    raw = {
        "sourceSize": list(im.size),
        "alpha0Ratio": round(float((a == 0).mean()), 4),
        "alpha255Ratio": round(float((a == 255).mean()), 4),
        "coreRatio": round(float((a >= ALPHA_HARDEN).mean()), 4),
        "edgeBandRatio": round(float(((a > ALPHA_KILL) & (a < ALPHA_HARDEN)).mean()), 4),
    }

    a = np.where(a >= ALPHA_HARDEN, 255.0, a)
    a = np.where(a <= ALPHA_KILL, 0.0, a)
    arr[:, :, 3] = a

    hardened = Image.fromarray(arr.astype(np.uint8), mode="RGBA")
    hardened = hardened.resize((SIZE, SIZE), Image.LANCZOS)
    return hardened, raw


def reposition_for_tail_swing(
    master: Image.Image,
    scale: float,
    margin_left: float,
) -> tuple[Image.Image, dict]:
    """把母版整体缩小并左移，给尾巴甩动留出空间。

    为什么会需要这一步（1:1 画幅实测踩的坑）：
    母版里猫的包围盒是 x[0.225, 0.949]——尾巴尖离右边界只剩 5%。
    而摇尾时尾尖摆幅可达**猫宽的 23.5%**，一甩就出画。
    实测 1:1 / 960×960 那支，121 帧里有 23 帧尾巴被切，
    右边界截面宽 64-78px，等于整条尾巴被齐刷刷切断。

    所以要么换宽画幅（但 16:9 只有 496px，分辨率砍半），
    要么在这里把猫缩小左移、腾出右侧空间——能保住 960 分辨率。
    """
    alpha = np.array(master)[:, :, 3]
    ys, xs = np.nonzero(alpha >= 128)
    x0, x1 = xs.min() / SIZE, xs.max() / SIZE
    y0, y1 = ys.min() / SIZE, ys.max() / SIZE
    cat_w, cat_h = x1 - x0, y1 - y0

    side = int(round(SIZE * scale))
    scaled = master.resize((side, side), Image.LANCZOS)

    canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    ox = int(round(margin_left * SIZE - x0 * side))
    oy = int(round((SIZE - cat_h * side) / 2 - y0 * side))
    sx0 = max(0, -ox)
    sy0 = max(0, -oy)
    canvas.paste(scaled.crop((sx0, sy0, side, side)), (max(0, ox), max(0, oy)))

    # 复核：算一下尾巴甩到最远时还剩多少余量
    swing = cat_w * TAIL_SWING_RATIO * scale
    right_edge = margin_left + cat_w * scale + swing
    vert_margin = (1.0 - cat_h * scale) / 2
    info = {
        "scale": scale,
        "marginLeft": margin_left,
        "catBoxNormalized": [round(x0, 4), round(y0, 4), round(x1, 4), round(y1, 4)],
        "catWidthAfterScale": round(cat_w * scale, 4),
        "catHeightAfterScale": round(cat_h * scale, 4),
        "tailSwingEstimate": round(swing, 4),
        "rightMarginAtFullSwing": round(1.0 - right_edge, 4),
        "verticalMargin": round(vert_margin, 4),
        "tailSwingRatioUsed": TAIL_SWING_RATIO,
    }
    return canvas, info


def composite_green(master: Image.Image) -> Image.Image:
    """把透明母版合成到纯绿幕上（RGB 输出）。"""
    bg = Image.new("RGB", master.size, GREEN)
    bg = bg.convert("RGBA")
    return Image.alpha_composite(bg, master).convert("RGB")


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

    # resolve 一次，后面 relative_to(ROOT) 才能正常工作
    out_dir = Path(args.outdir).resolve() if args.outdir else OUT_DIR
    out_dir.mkdir(parents=True, exist_ok=True)
    report = {"size": SIZE, "green": list(GREEN), "candidates": []}

    if args.master:
        chosen = Path(args.master).resolve()
        if not chosen.is_file():
            raise SystemExit(f"[缺输入] 母版不存在: {chosen}")
        candidates = [(chosen.stem, chosen)]
    else:
        candidates = CANDIDATES

    for name, path in candidates:
        if not path.is_file():
            print(f"[SKIP] 缺少母版: {path}")
            continue
        master, raw = prepare_master(path)
        mask = np.array(master)[:, :, 3] >= 128
        stats = analyze(mask)
        entry = {"style": name, "source": str(path.relative_to(ROOT)), **raw, **stats}
        report["candidates"].append(entry)
        print(f"[分析] {name}: 主体占比={stats['foregroundRatio']:.3f} "
              f"细结构={stats['thinStructureRatio']} 边缘带={raw['edgeBandRatio']:.4f}")

    if not report["candidates"]:
        print("[FAIL] 没有可用母版")
        return 1

    # 选边缘带最窄、且细结构可测的作为首帧
    best = sorted(report["candidates"], key=lambda e: (e["edgeBandRatio"]), reverse=False)[0]
    report["selected"] = best["style"]
    print(f"\n[选定] 首帧母版 = {best['style']}")

    src_path = next(p for n, p in candidates if n == best["style"])
    master, _ = prepare_master(src_path)

    # 缩小并左移，给尾巴甩动腾出右侧空间（1:1 画幅下不这么做会切掉整条尾巴）
    if args.scale < 1.0:
        master, fit = reposition_for_tail_swing(master, args.scale, args.margin_left)
        report["tailSwingFit"] = fit
        print(f"[取景] 猫缩放 {args.scale:.2f}、左边距 {args.margin_left:.2f}")
        print(f"       尾巴甩到最远时右侧余量 {fit['rightMarginAtFullSwing']*100:.1f}%"
              f"  上下余量 {fit['verticalMargin']*100:.1f}%")
        if fit["rightMarginAtFullSwing"] < 0.05:
            print("[警告] 右侧余量不足 5%，尾巴仍可能出画，请调小 --scale")

    master_path = out_dir / "母版-1024.png"
    master.save(master_path)

    frame = composite_green(master)
    # JPG：兼容性最好，作为默认首帧（已关闭色彩二次采样，质量 95）
    frame_path = out_dir / "绿幕首帧-1024.jpg"
    frame.save(frame_path, quality=95, subsampling=0)
    # PNG：无损备选，若 Seedance 支持 PNG 上传应优先使用
    png_path = out_dir / "绿幕首帧-1024.png"
    frame.save(png_path)

    # 首帧自检：绿幕纯度
    farr = np.array(frame).astype(np.int16)
    fg = (np.array(master)[:, :, 3] >= 128)
    bg_pixels = farr[~fg]
    green_exact = int(np.all(bg_pixels == np.array(GREEN, dtype=np.int16), axis=1).sum())
    report["firstFrame"] = {
        "path": str(frame_path.relative_to(ROOT)),
        "losslessPath": str(png_path.relative_to(ROOT)),
        "masterPath": str(master_path.relative_to(ROOT)),
        "size": list(frame.size),
        "backgroundGreenExactRatio": round(green_exact / max(1, bg_pixels.shape[0]), 6),
        "backgroundPixelCount": int(bg_pixels.shape[0]),
    }
    print(f"[输出] {frame_path}")
    print(f"[自检] 背景纯绿像素占比 = {report['firstFrame']['backgroundGreenExactRatio']:.6f}")

    (out_dir / "首帧分析.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

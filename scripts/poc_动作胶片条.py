# -*- coding: utf-8 -*-
"""把多个动作版本的「局部放大帧」排成胶片条，用于人眼核对序列形状。

为什么需要这个：
  机械自检只能证明"身体没动、变化像素都在目标矩形内"，证明不了"动作看起来对不对"。
  而预览页是动态的，一闪而过反而看不清每一帧的形状。胶片条把每一帧并排钉死，
  一眼就能看出：闭合够不够深、睁的过程有几帧、末帧回锚点跳变有多大。

只依赖 Pillow，不依赖 cv2。放大用 NEAREST，保像素硬边不被插值糊掉。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_动作胶片条.py \
      --base   output/.../04-帧序列-呼吸循环/frames/f0000.png \
      --dirs   output/.../06-眨眼/01-帧序列/frames \
               output/.../06-眨眼/02-帧序列-8帧-336ms/frames \
      --crop   0.1412,0.1837,0.2415,0.1378 \
      --zoom   4 \
      --out    output/.../06-眨眼/_预览对比/胶片条.png
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
BG = (24, 26, 30)
GAP = 6


def load_rgba(p: Path) -> Image.Image:
    return Image.open(p).convert("RGBA")


def on_bg(im: Image.Image, bg=(0, 0, 0)) -> Image.Image:
    """合成到纯色底上，方便看边缘（透明区直接变背景色）。"""
    out = Image.new("RGB", im.size, bg)
    out.paste(im, (0, 0), im)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description="动作帧胶片条（多版本横向对照）")
    ap.add_argument("--base", required=True, help="锚点帧（呼吸 f0000）")
    ap.add_argument("--dirs", nargs="+", required=True, help="各版本的帧目录")
    ap.add_argument("--crop", required=True, help="归一化裁剪区 x,y,w,h")
    ap.add_argument("--zoom", type=int, default=4)
    ap.add_argument("--bg", default="black", choices=["black", "white", "checker"])
    ap.add_argument("--out", required=True)
    ap.add_argument("--scale", type=float, default=1.0,
                    help="最终整图缩放（0~1，便于塞进一张图里看）")
    args = ap.parse_args()

    x, y, w, h = (float(t) for t in args.crop.split(","))
    anchor = load_rgba(Path(args.base))
    W0, H0 = anchor.size
    box = (int(x * W0), int(y * H0), int((x + w) * W0), int((y + h) * H0))
    cw, ch = box[2] - box[0], box[3] - box[1]

    def crop_zoom(im: Image.Image) -> Image.Image:
        c = im.crop(box)
        if args.bg == "checker":
            cell = max(4, cw // 16)
            bg_img = Image.new("RGB", (cw, ch), (238, 238, 238))
            d = ImageDraw.Draw(bg_img)
            for yy in range(0, ch, cell):
                for xx in range(0, cw, cell):
                    if ((xx // cell) + (yy // cell)) % 2:
                        d.rectangle([xx, yy, xx + cell - 1, yy + cell - 1], fill=(198, 198, 198))
            out = bg_img
            out.paste(c, (0, 0), c)
            c = out
        else:
            c = on_bg(c, (0, 0, 0) if args.bg == "black" else (255, 255, 255))
        return c.resize((cw * args.zoom, ch * args.zoom), Image.NEAREST)

    versions = []
    for d in args.dirs:
        p = Path(d)
        fr = sorted(p.glob("f*.png"))
        if not fr:
            raise SystemExit(f"[缺帧] {p}")
        versions.append((p.parent.name, fr))
    maxn = max(len(f) for _, f in versions)

    tw, th = cw * args.zoom, ch * args.zoom
    # 布局：每版一行；行首留一格放锚点帧（"睁眼"基准），后面是动作各帧
    rows = len(versions)
    cols = maxn + 1
    pad = GAP
    img_w = cols * tw + (cols + 1) * pad
    img_h = rows * th + (rows + 1) * pad
    canvas = Image.new("RGB", (img_w, img_h), BG)
    dr = ImageDraw.Draw(canvas)

    anchor_cell = crop_zoom(anchor)
    for r, (tag, fr) in enumerate(versions):
        ry = pad + r * (th + pad)
        cx = pad
        canvas.paste(anchor_cell, (cx, ry))
        dr.rectangle([cx - 1, ry - 1, cx + tw, ry + th], outline=(70, 78, 90))
        for i, fp in enumerate(fr):
            cx = pad + (i + 1) * (tw + pad)
            canvas.paste(crop_zoom(load_rgba(fp)), (cx, ry))
            # 末帧描绿框：提示"这一帧之后直接回锚点"，跳变大不大重点看它
            outline = (110, 190, 130) if i == len(fr) - 1 else (70, 78, 90)
            dr.rectangle([cx - 1, ry - 1, cx + tw, ry + th], outline=outline)
        print(f"[胶片条] {tag}: {len(fr)} 帧 / {len(fr) * 42}ms　末帧绿框=回锚点前最后一帧")

    if args.scale != 1.0:
        canvas = canvas.resize((int(img_w * args.scale), int(img_h * args.scale)), Image.LANCZOS)

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(out)
    print(f"[输出] {out}  ({canvas.size[0]}×{canvas.size[1]})")
    print(f"[读图] 每版一行：首格=锚点(睁眼基准)，其后=动作第1..N帧，末帧绿框。")
    return 0


if __name__ == "__main__":
    sys.exit(main())

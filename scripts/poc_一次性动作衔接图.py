# -*- coding: utf-8 -*-
"""一次性动作衔接证据图：向人证明「触发瞬间不跳变」。

产出两张图
----------
1. `<out>/01-动作时间线.png`
   全动作等距采样拼图（棋盘格底），用来肉眼确认动作真的发生了。

2. `<out>/02-衔接对比.png`
   三格并排：默认循环锚点帧 | 一次性动作首帧 | 一次性动作末帧。
   下面一行是「锚点 vs 首帧」「锚点 vs 末帧」的差值热图（越黑越像）。
   一次性动作是在默认循环边界触发的，进出两个接缝都要对得上；
   05-silver-tabby 就是因为只管了动作本身、没管衔接，触发时猫跳了 27px。

用法
----
    D:/DevTools/Python312/python.exe scripts/poc_一次性动作衔接图.py \
        --frames output/.../10-打哈欠/02-帧序列/frames \
        --anchor apps/desktop/public/builtin-pets/04-warm-brown-tabby/frames/idle-combo/f0000.png \
        --out output/.../10-打哈欠/04-证据 [--cols 16]
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

import cv2
import numpy as np

TILE = 16          # 棋盘格边长
BG_DARK = 190      # 棋盘格深格
BG_LIGHT = 235     # 棋盘格浅格
PAD = 8
LABEL_H = 26


def imread_rgba(p: Path) -> np.ndarray:
    # cv2.imdecode 返回 BGR(A)，与 cv2.imencode 顺序对齐，不要转 RGBA。
    img = cv2.imdecode(np.fromfile(str(p), np.uint8), cv2.IMREAD_UNCHANGED)
    assert img is not None, f"读取失败 {p}"
    if img.shape[2] == 3:
        img = cv2.cvtColor(img, cv2.COLOR_BGR2BGRA)
    return img


def checker(size: int) -> np.ndarray:
    yy, xx = np.indices((size, size))
    c = ((xx // TILE + yy // TILE) % 2 == 0)
    bg = np.where(c, BG_LIGHT, BG_DARK).astype(np.uint8)
    return np.dstack([bg, bg, bg, np.full((size, size), 255, np.uint8)])


def composite(fg: np.ndarray, bg: np.ndarray) -> np.ndarray:
    """fg over bg，fg 必须居中且尺寸 <= bg。"""
    h, w = fg.shape[:2]
    bh, bw = bg.shape[:2]
    y0, x0 = (bh - h) // 2, (bw - w) // 2
    a = (fg[:, :, 3:4].astype(np.float32) / 255.0)
    out = bg.copy().astype(np.float32)
    roi = out[y0:y0 + h, x0:x0 + w, :3]
    out[y0:y0 + h, x0:x0 + w, :3] = fg[:, :, :3].astype(np.float32) * a + roi * (1 - a)
    return out.astype(np.uint8)


def label_bar(width: int, text: str) -> np.ndarray:
    bar = np.full((LABEL_H, width, 3), 255, np.uint8)
    cv2.putText(bar, text, (8, 18), cv2.FONT_HERSHEY_SIMPLEX, 0.5, (20, 20, 20), 1,
                cv2.LINE_AA)
    return bar


def hstack(tiles: list[np.ndarray], labels: list[str]) -> np.ndarray:
    h = max(t.shape[0] for t in tiles)
    rows = []
    for t in tiles:
        pad = np.full((h - t.shape[0], t.shape[1], 3), 255, np.uint8)
        rows.append(np.vstack([t, pad]) if t.shape[0] < h else t)
    body = np.hstack(rows)
    bars = [label_bar(t.shape[1], lb) for t, lb in zip(tiles, labels)]
    bar = np.hstack(bars) if bars else None
    return np.vstack([body, bar]) if bar is not None else body


def grid(tiles: list[np.ndarray], cols: int) -> np.ndarray:
    rows = [tiles[i:i + cols] for i in range(0, len(tiles), cols)]
    h = max(t.shape[0] for t in tiles)
    w = max(t.shape[1] for t in tiles)
    out = []
    for r in rows:
        line = []
        for t in r:
            pad = np.full((h, w, 3), 255, np.uint8)
            pad[:t.shape[0], :t.shape[1]] = t
            line.append(pad)
        # 补齐列数，保持网格整齐
        while len(line) < cols:
            line.append(np.full((h, w, 3), 255, np.uint8))
        out.append(np.hstack(line))
    return np.vstack(out)


def diff_heat(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    """两帧主体区域的 RGB 差异热图（黑白：白=差异大）。"""
    m = ((a[:, :, 3] >= 128) | (b[:, :, 3] >= 128))
    d = np.abs(a[:, :, :3].astype(np.int16) - b[:, :, :3].astype(np.int16)).max(axis=2)
    d = np.clip(d * 3, 0, 255).astype(np.uint8)   # 放大 3 倍便于肉眼
    d[~m] = 0
    return cv2.cvtColor(d, cv2.COLOR_GRAY2BGR)


def main() -> int:
    ap = argparse.ArgumentParser(description="一次性动作衔接证据图")
    ap.add_argument("--frames", required=True, help="一次性动作帧目录")
    ap.add_argument("--anchor", required=True, help="默认循环锚点帧")
    ap.add_argument("--out", required=True, help="输出目录")
    ap.add_argument("--cols", type=int, default=16, help="时间线每行帧数")
    ap.add_argument("--action-name", default="one-shot", help="动作名（用于图上标签）")
    args = ap.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    names = sorted(Path(args.frames).glob("f*.png"))
    assert names, f"无帧: {args.frames}"
    imgs = [imread_rgba(p) for p in names]
    n = len(imgs)
    size = imgs[0].shape[0]
    anchor = imread_rgba(Path(args.anchor))
    bg = checker(size)

    # 1) 时间线
    idx = np.linspace(0, n - 1, args.cols).astype(int)
    tiles = [composite(imgs[i], bg)[:, :, :3] for i in idx]
    line = grid(tiles, args.cols)
    cap = label_bar(line.shape[1],
                    f"{args.action_name} 时间线 f{idx[0]:04d}..f{idx[-1]:04d}  ({n} 帧 @42ms = {n*42}ms)")
    p1 = out / "01-动作时间线.png"
    cv2.imencode(".png", np.vstack([line, cap]))[1].tofile(str(p1))
    print(f"[输出] {p1}")

    # 2) 衔接对比
    first, last = imgs[0], imgs[-1]
    trio = [composite(anchor, bg)[:, :, :3],
            composite(first, bg)[:, :, :3],
            composite(last, bg)[:, :, :3]]
    top = hstack(trio, ["idle-combo 锚点 f0000", f"{args.action_name} 首帧 f0000", f"{args.action_name} 末帧 f%04d" % (n - 1)])
    d1 = diff_heat(anchor, first)
    d2 = diff_heat(anchor, last)
    blank = np.zeros_like(d1)
    bot = hstack([d1, d2, blank],
                 ["差值 锚点vs首帧 (x3放大)", "差值 锚点vs末帧 (x3放大)", ""])
    p2 = out / "02-衔接对比.png"
    cv2.imencode(".png", np.vstack([top, bot]))[1].tofile(str(p2))
    print(f"[输出] {p2}")

    def iou(a: np.ndarray, b: np.ndarray) -> float:
        ma, mb = a[:, :, 3] >= 128, b[:, :, 3] >= 128
        u = np.count_nonzero(ma | mb)
        return float(np.count_nonzero(ma & mb) / u) if u else 0.0

    print(f"[指标] 首帧/锚点 IoU={iou(first, anchor):.4f}  末帧/锚点 IoU={iou(last, anchor):.4f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

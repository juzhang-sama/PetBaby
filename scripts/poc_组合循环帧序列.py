# -*- coding: utf-8 -*-
"""组合循环帧序列：抠像帧 -> 无缝循环帧序列。

输入：09-组合循环/00-抠像/frames（N 帧透明 PNG，已 color-match 母版）
输出：09-组合循环/01-帧序列/frames（resize 到目标尺寸 + 末段替换首帧做无缝循环）

循环无缝策略：视频首尾本就有 8-12 的姿态/耳朵差异（Seedance 视频内重绘），
循环点会跳。用「末段替换首帧」兜底：把末尾 TAIL_REPLACE 帧替换成首帧，
让循环点 f[last] == f[0] 严格一致；跳变集中到替换段前一帧。
可选 --blend 用交叉淡化替代硬替换，把跳变分散成 K 帧渐变（更柔和，但末段会有
首帧姿态的残影）。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_组合循环帧序列.py \
      --src output/.../09-组合循环/00-抠像/frames \
      --out output/.../09-组合循环/01-帧序列 \
      --size 588 [--blend 5]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
FRAME_MS = 42


def imread_rgba(p: Path) -> np.ndarray:
    buf = np.fromfile(str(p), np.uint8)
    img = cv2.imdecode(buf, cv2.IMREAD_UNCHANGED)
    assert img is not None, f"读取失败 {p}"
    return img


def imwrite_rgba(p: Path, img: np.ndarray) -> None:
    ok, buf = cv2.imencode(".png", img)
    assert ok
    buf.tofile(str(p))


def sha256_of(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser(description="组合循环帧序列后处理")
    ap.add_argument("--src", required=True, help="抠像 frames 目录")
    ap.add_argument("--out", required=True, help="输出目录")
    ap.add_argument("--size", type=int, default=588, help="输出边长")
    ap.add_argument("--blend", type=int, default=0,
                    help="交叉淡化帧数（>0 时末段用渐变替代硬替换）")
    args = ap.parse_args()

    src = Path(args.src)
    out = Path(args.out)
    frames_out = out / "frames"
    frames_out.mkdir(parents=True, exist_ok=True)

    names = sorted(src.glob("f*.png"))
    assert names, f"源目录无帧: {src}"
    imgs = [imread_rgba(p) for p in names]
    n = len(imgs)

    # resize
    if args.size and imgs[0].shape[0] != args.size:
        imgs = [cv2.resize(im, (args.size, args.size), interpolation=cv2.INTER_AREA)
                for im in imgs]
        print(f"[缩放] {n} 帧 -> {args.size}x{args.size}")

    anchor = imgs[0].copy()

    if args.blend > 0:
        k = min(args.blend, n - 1)
        for j in range(1, k + 1):
            idx = n - 1 - k + j  # 倒数第 k..1 帧
            w = j / (k + 1)      # 渐强到首帧
            imgs[idx] = (imgs[idx].astype(np.float32) * (1 - w)
                         + anchor.astype(np.float32) * w).astype(np.uint8)
        imgs[-1] = anchor.copy()
        print(f"[无缝] 末段 {k} 帧交叉淡化到首帧")
    else:
        # 末 2 帧硬替换成首帧，循环点严格一致
        for idx in (n - 1, n - 2):
            imgs[idx] = anchor.copy()
        print(f"[无缝] 末 2 帧硬替换成首帧（循环点 f{n-1:04d} == f0000）")

    files = []
    for i, im in enumerate(imgs):
        fp = frames_out / f"f{i:04d}.png"
        imwrite_rgba(fp, im)
        files.append({"role": "frame", "relativePath": f"frames/f{i:04d}.png",
                      "sha256": sha256_of(fp)})
    print(f"[输出] {n} 帧 x {FRAME_MS}ms = {n * FRAME_MS}ms -> {frames_out}")

    # 自检
    first = imread_rgba(frames_out / "f0000.png")
    last = imread_rgba(frames_out / f"f{n-1:04d}.png")
    d = int(np.abs(first.astype(np.int16) - last.astype(np.int16)).max())
    print(f"[自检] 首尾最大像素差 {d} (应=0) {'PASS' if d == 0 else 'FAIL'}")

    areas = [int((im[:, :, 3] >= 128).sum()) for im in imgs]
    areas = np.array(areas, dtype=np.float64)
    base = areas[0]
    print(f"[自检] 前景面积 base={int(base)} 最大波动 "
          f"{float(np.abs(areas - base).max() / base):.4%}")

    touched = [i for i, im in enumerate(imgs)
               if (im[:, :, 3] >= 128)[:, :2].any() or (im[:, :, 3] >= 128)[:, -2:].any()
               or (im[:, :, 3] >= 128)[:2, :].any() or (im[:, :, 3] >= 128)[-2:, :].any()]
    print(f"[自检] 触边帧 {len(touched)}/{n} {'PASS' if not touched else 'FAIL: ' + str(touched[:10])}")

    manifest = {
        "renderer": "frame-sequence-v1",
        "actionId": "idle-combo",
        "loop": True,
        "frameDurationMs": FRAME_MS,
        "frameCount": n,
        "durationMs": n * FRAME_MS,
        "sourceVideo": "03-视频/05-组合循环-呼吸眨眼摇尾.mp4",
        "size": args.size,
        "loopSeam": "tail-replace" if not args.blend else f"crossfade-{args.blend}",
        "files": files,
    }
    (out / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"[输出] {out / 'manifest.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

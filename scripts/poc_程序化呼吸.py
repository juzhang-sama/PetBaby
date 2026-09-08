# -*- coding: utf-8 -*-
"""程序化生成呼吸循环帧序列。

为什么不用 AI 生成呼吸：
  1. Seedance 把呼吸做成**整体膨胀**（实测轮廓面积涨 3.8%），真实呼吸是胸腹局部起伏、
     整体轮廓几乎不变，所以看着像"大口喘气"。
  2. 它不遵守时间轴（要求 5 秒 2 个周期，实际给了 5 秒 1 个）。
  3. 重新生成大概率还是同样的问题——这次已经证明过一次。

为什么不后处理压幅度：
  实测光流压幅度（k=0.5）锐利度只剩 59%，且轮廓面积波动反而从 4.30% 升到 5.33%。
  呼吸是膨胀形变，光流对这种形变估计很差。走不通。

程序化方案（与像素路线 make_breath 同源：横向重采样 + 空间权重）：
  - 锐利度保留 97~101%（几乎无损）
  - 面积增幅 ≈0（局部起伏，不是整体膨胀）
  - 幅度、周期、吸呼比全部精确可控
  - 首尾由周期函数保证完全一致，循环无缝

用法：
  D:/DevTools/Python312/python.exe scripts/poc_程序化呼吸.py \
      --anchor output/.../04-帧序列/frames/f000.png \
      --out    output/.../04-帧序列-呼吸循环 \
      --frames 60 --amplitude 0.012 --inhale-ratio 0.4
"""
from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]

# 胸腹区间（按主体高度归一化）。两端用 smoothstep 收口，
# 保证头部、四肢、尾巴权重为 0 —— 只有胸腹在动。
CHEST_TOP_IN = 0.34      # 胸腹上边界：从此处开始渐入
CHEST_TOP_FULL = 0.52    # 此处达到满权重
CHEST_BOTTOM_FULL = 0.84 # 此处开始渐出
CHEST_BOTTOM_OUT = 0.96  # 此处权重归零

PAD = 24                 # 形变时向外扩边，避免拉出画布


def imread_u(path: Path) -> np.ndarray:
    return cv2.imdecode(np.fromfile(str(path), dtype=np.uint8), cv2.IMREAD_UNCHANGED)


def imwrite_u(path: Path, img: np.ndarray) -> None:
    ok, buf = cv2.imencode(".png", img)
    if not ok:
        raise SystemExit(f"[写图失败] {path}")
    buf.tofile(str(path))


def smoothstep(e0: float, e1: float, x: np.ndarray) -> np.ndarray:
    t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
    return t * t * (3 - 2 * t)


def breath_weights(alpha: np.ndarray):
    """返回 (空间权重, 主体中心 x, bbox)。"""
    ys, xs = np.nonzero(alpha > 0.5)
    if ys.size == 0:
        raise SystemExit("[错误] 锚点帧没有主体像素")
    y0, y1, x0, x1 = ys.min(), ys.max(), xs.min(), xs.max()
    bh = float(y1 - y0)
    cy = (y0 + y1) / 2.0
    cx = (x0 + x1) / 2.0
    ny = (np.arange(alpha.shape[0], dtype=np.float32)[:, None] - y0) / bh
    w = smoothstep(CHEST_TOP_IN, CHEST_TOP_FULL, ny) * (
        1.0 - smoothstep(CHEST_BOTTOM_FULL, CHEST_BOTTOM_OUT, ny)
    )
    w = np.repeat(w, alpha.shape[1], axis=1) * alpha
    return w, float(cx), (y0, y1, x0, x1)


def breath_curve(p: np.ndarray, inhale_ratio: float) -> np.ndarray:
    """呼吸位移曲线，p ∈ [0,1)。吸气略快、呼气略慢。

    首尾值均为 0 且一阶导为 0，保证循环无缝。
    """
    p = np.asarray(p, dtype=np.float64)
    out = np.empty_like(p)
    inh = p < inhale_ratio
    t = p[inh] / inhale_ratio
    out[inh] = (1.0 - np.cos(np.pi * t)) / 2.0
    t = (p[~inh] - inhale_ratio) / (1.0 - inhale_ratio)
    out[~inh] = (1.0 + np.cos(np.pi * t)) / 2.0
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description="程序化生成呼吸循环帧序列")
    ap.add_argument("--anchor", required=True, help="静止锚点帧（透明 PNG）")
    ap.add_argument("--out", required=True, help="输出目录")
    ap.add_argument("--frames", type=int, default=60, help="循环帧数（默认 60 = 2.5s@24fps）")
    ap.add_argument("--fps", type=float, default=24.0)
    ap.add_argument("--amplitude", type=float, default=0.012, help="胸腹横向扩张幅度（默认 1.2%）")
    ap.add_argument("--inhale-ratio", type=float, default=0.4, help="吸气占周期比例（默认 0.4，呼气更慢）")
    ap.add_argument("--manifest", help="参考 manifest（复制其文件清单结构），可选")
    args = ap.parse_args()

    anchor_path = Path(args.anchor)
    if not anchor_path.is_file():
        raise SystemExit(f"[缺输入] 锚点帧不存在: {anchor_path}")
    out_dir = Path(args.out).resolve()
    frames_dir = out_dir / "frames"
    if frames_dir.exists():
        shutil.rmtree(frames_dir)
    frames_dir.mkdir(parents=True, exist_ok=True)

    src = imread_u(anchor_path)
    if src.shape[2] != 4:
        raise SystemExit("[错误] 锚点帧必须是 RGBA PNG")
    h, w = src.shape[:2]
    canvas = np.zeros((h + 2 * PAD, w + 2 * PAD, 4), np.uint8)
    canvas[PAD:PAD + h, PAD:PAD + w] = src
    H, W = canvas.shape[:2]

    alpha = canvas[:, :, 3].astype(np.float32) / 255.0
    w_sp, cx, (y0, y1, x0, x1) = breath_weights(alpha)
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)

    print(f"[锚点] {anchor_path.name}  {w}x{h}")
    print(f"[主体] bbox y[{y0},{y1}] x[{x0},{x1}]  中心x={cx:.0f}  胸腹权重峰值={w_sp.max():.3f}")
    print(f"[参数] 帧数={args.frames} fps={args.fps} 周期={args.frames/args.fps:.2f}s "
          f"幅度={args.amplitude*100:.1f}% 吸气占比={args.inhale_ratio}")

    cycle_ms = round(args.frames / args.fps * 1000)
    print(f"[周期] {cycle_ms} ms  -> 动作插入粒度 = {args.frames/args.fps:.2f} 秒")

    phases = np.arange(args.frames) / args.frames
    curve = breath_curve(phases, args.inhale_ratio) * args.amplitude

    def area_of(img): return int((img[:, :, 3] > 128).sum())
    def sharp_of(img): return cv2.Laplacian(img[:, :, 3].astype(np.float32), cv2.CV_32F).var()

    base_area, base_sharp = area_of(canvas), sharp_of(canvas)
    areas, sharps = [], []

    for i, d in enumerate(curve):
        dx = d * (xx - cx) * w_sp
        map_x = (xx - dx).astype(np.float32)
        map_y = yy.astype(np.float32)
        out = np.zeros_like(canvas)
        for c in range(4):
            out[:, :, c] = cv2.remap(canvas[:, :, c], map_x, map_y, cv2.INTER_LINEAR,
                                     borderMode=cv2.BORDER_CONSTANT, borderValue=0)
        # 裁回原始画布尺寸与位置，保证与其他动作单元取景一致
        out = out[PAD:PAD + h, PAD:PAD + w]
        imwrite_u(frames_dir / f"f{i:04d}.png", out)
        areas.append(area_of(out))
        sharps.append(sharp_of(out))

    lo, hi = min(areas), max(areas)
    print(f"\n[输出] {args.frames} 帧 -> {frames_dir}")
    print(f"[自检] 轮廓面积波动 {(hi-lo)/np.mean(areas)*100:.2f}%   (验收阈值 ≤6%)")
    print(f"[自检] 边缘锐利度 {np.mean(sharps):.1f}  (锚点 {base_sharp:.1f}，"
          f"保留 {np.mean(sharps)/base_sharp*100:.1f}%)")

    # 循环首尾一致性
    f0 = imread_u(frames_dir / "f0000.png")
    fl = imread_u(frames_dir / f"f{args.frames-1:04d}.png")
    diff = np.abs(fl.astype(np.float32) - f0.astype(np.float32))
    print(f"[自检] 循环接缝（末帧 vs 首帧）平均差 {diff.mean():.3f} 最大差 {diff.max():.0f}")

    manifest = {
        "schemaVersion": 6,
        "renderer": "frame-sequence-v1",
        "petId": out_dir.parent.name,
        "variantId": "breath-programmatic",
        "displayName": "呼吸循环（程序化）",
        "species": "cat",
        "baseImage": f"frames/f0000.png",
        "defaultAction": "breath",
        "anchorPolicy": "fixed",
        "actions": [{
            "actionId": "breath",
            "loop": True,
            "frameDurationMs": round(1000 / args.fps),
            "frames": [f"frames/f{i:04d}.png" for i in range(args.frames)],
        }],
        "semantics": {"idle": "breath"},
        "files": [],
        "_provenance": {
            "generator": "scripts/poc_程序化呼吸.py",
            "anchor": str(anchor_path),
            "amplitude": args.amplitude,
            "inhaleRatio": args.inhale_ratio,
            "cycleMs": cycle_ms,
            "note": "程序化生成，非 AI 视频。首尾由周期函数保证完全一致。",
        },
    }
    (out_dir / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"[输出] manifest -> {out_dir / 'manifest.json'}")

    print(f"\n[下一步] 预览：")
    print(f"    D:/DevTools/Python312/python.exe scripts/poc_预览.py --frames {frames_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

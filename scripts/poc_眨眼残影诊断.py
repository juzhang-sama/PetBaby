# -*- coding: utf-8 -*-
"""诊断眨眼中间帧的"残影"：到底是物理半闭，还是睁/闭两个状态的叠加。

背景：
  老王反馈"眼睛已经闭上了，但睁眼的状态还有残留的瞬间，像残影一样别扭"。
  怀疑根因是合成用了线性 alpha 混合：mid = open*(1-m) + closed*m。
  这个公式在中间 m 上会让**睁和闭两个状态同时可见** —— 而真实眨眼是眼睑扫过，
  任何一瞬间眼睛只有一个物理状态（要么睁、要么被盖住），不存在"既睁又闭"。

诊断原理（不靠推理，靠反解）：
  从三张图 open / mid / closed 反解每像素的混合权重
      m(x,y) = (mid - open) / (closed - open)      （只在 closed != open 处有定义）
  - 若整块眼周的 m 是**常数**（≈该帧的设计强度）→ 纯线性混合 → 叠加态，残影成立
  - 若 m 有**空间结构**（上部≈1 被眼睑盖住、下部≈0 还睁着）→ 物理眼睑扫描，没问题

再统计 m 的直方图：
  - 纯混合 → m 单峰，峰值≈帧强度
  - 物理扫描 → m 双峰（≈0 和 ≈1），中间几乎没有像素

用法：
  D:/DevTools/Python312/python.exe scripts/poc_眨眼残影诊断.py \
      --open   output/.../04-帧序列-呼吸循环/frames/f0000.png \
      --seq    output/.../06-眨眼/01-帧序列/frames \
      --rect   113,184,88,220 \
      --out    output/.../06-眨眼/_诊断
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

import cv2
import numpy as np

MIN_DENOM = 24.0   # |closed-open| 小于此值的像素不参与反解（两图几乎相同，m 无意义）
GHOST_LO = 0.20    # m 落在 (GHOST_LO, GHOST_HI) 视为"叠加态像素"
GHOST_HI = 0.80


def imread_u(path: Path) -> np.ndarray:
    img = cv2.imdecode(np.fromfile(str(path), dtype=np.uint8), cv2.IMREAD_UNCHANGED)
    if img is None:
        raise SystemExit(f"[读图失败] {path}")
    return img


def main() -> int:
    ap = argparse.ArgumentParser(description="眨眼残影诊断")
    ap.add_argument("--open", required=True, help="睁眼基准（呼吸锚点帧 f0000）")
    ap.add_argument("--seq", required=True, help="眨眼帧序列目录")
    ap.add_argument("--rect", required=True, help="眼周替换矩形 y0,y1,x0,x1")
    ap.add_argument("--out", required=True, help="输出目录")
    args = ap.parse_args()

    y0, y1, x0, x1 = (int(t) for t in args.rect.split(","))
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    open_im = imread_u(Path(args.open)).astype(np.float32)
    frames = sorted(Path(args.seq).glob("f*.png"))
    if not frames:
        raise SystemExit(f"[缺帧] {args.seq}")

    # 闭眼基准 = 序列里与锚点差异最大的那帧（强度 1.0）
    diffs = []
    for p in frames:
        f = imread_u(p).astype(np.float32)
        d = np.abs(f[:, :, :3] - open_im[:, :, :3]).max(axis=2)
        diffs.append((float(d[y0:y1, x0:x1].mean()), p))
    diffs.sort(reverse=True)
    closed_im = imread_u(diffs[0][1]).astype(np.float32)
    print(f"[闭眼基准] {diffs[0][1].name}  （眼周平均差异 {diffs[0][0]:.1f}）")

    O = open_im[y0:y1, x0:x1, :3]
    C = closed_im[y0:y1, x0:x1, :3]
    denom = np.abs(C - O).max(axis=2)
    valid = denom >= MIN_DENOM
    print(f"[有效像素] {int(valid.sum())} / {valid.size} "
          f"(|closed-open| >= {MIN_DENOM:.0f})")

    print("\n逐帧反解混合权重 m —— 常数=线性混合(有残影)，有空间结构=物理眼睑扫描")
    print(f"{'帧':<10}{'设计强度':>10}{'m均值':>10}{'m标准差':>10}{'m中位数':>10}"
          f"{'叠加态像素占比':>16}{'判定':>10}")
    rows = []
    for p in frames:
        F = imread_u(p).astype(np.float32)[y0:y1, x0:x1, :3]
        num = (F - O).max(axis=2)
        m = np.where(valid, num / np.maximum(denom, 1e-6), np.nan)
        mv = m[valid]
        # 该帧的"设计强度"用 m 的中位数估计（若纯混合，m 处处等于设计强度）
        design = float(np.median(mv))
        ghost = float(np.mean((mv > GHOST_LO) & (mv < GHOST_HI)))
        verdict = "叠加态" if ghost > 0.25 else ("物理" if ghost < 0.05 else "混合不清")
        rows.append((p.name, design, float(mv.mean()), float(mv.std()), ghost))
        print(f"{p.name:<10}{design:>10.3f}{mv.mean():>10.3f}{mv.std():>10.3f}"
              f"{design:>10.3f}{ghost*100:>15.1f}%{verdict:>10}")

    # 直方图：纯混合→单峰；物理扫描→双峰
    print("\n混合权重 m 的直方图（0=完全睁，1=完全闭）")
    bins = np.linspace(0, 1, 11)
    for p in frames:
        F = imread_u(p).astype(np.float32)[y0:y1, x0:x1, :3]
        num = (F - O).max(axis=2)
        m = np.where(valid, num / np.maximum(denom, 1e-6), np.nan)
        mv = m[valid]
        hist, _ = np.histogram(mv, bins=bins)
        hist = hist / max(1, hist.sum())
        bar = " ".join(f"{h*100:4.0f}" for h in hist)
        print(f"  {p.name}  [{bar}]")

    # 可视化：把"叠加态像素"标红，叠加在中间帧的眼部放大图上
    mids = [r for r in rows if GHOST_LO < r[1] < GHOST_HI]
    if mids:
        target = mids[len(mids) // 2][0]
        F = imread_u(Path(args.seq) / target).astype(np.float32)
        num = (F[y0:y1, x0:x1, :3] - O).max(axis=2)
        m = np.where(valid, num / np.maximum(denom, 1e-6), 0.0)
        ghost_mask = (m > GHOST_LO) & (m < GHOST_HI)
        vis = F[y0:y1, x0:x1, :3].copy()
        vis[ghost_mask] = (vis[ghost_mask] * 0.35 + np.array([0, 0, 255]) * 0.65)
        vis_big = cv2.resize(vis.astype(np.uint8), None, fx=6, fy=6, interpolation=cv2.INTER_NEAREST)
        cv2.imencode(".png", vis_big)[1].tofile(str(out_dir / f"残影热力图-{target}.png"))
        print(f"\n[输出] 残影热力图（红色=叠加态像素）-> {out_dir / f'残影热力图-{target}.png'}")
        print(f"       {target}：{ghost_mask.mean()*100:.1f}% 的眼周像素处于'既不睁也不闭'的叠加态")

    print("\n结论提示：")
    print("  若 m 标准差 ≈ 0 且叠加态占比很高 → 纯线性混合，残影成立，需改物理眼睑扫描")
    print("  若 m 呈上下分层（上部≈1 下部≈0）   → 已是物理扫描，残影另有原因")
    return 0


if __name__ == "__main__":
    sys.exit(main())

# -*- coding: utf-8 -*-
"""
视频速诊：等间隔拼图 + 实测背景色 + 运动时间线
用于新视频到手时第一眼判断"猫到底动了没有、背景是不是标准绿幕"。
MEMORY: 机械指标全绿 ≠ 用户能看到，必须肉眼过一遍。
"""
import argparse
import cv2
import numpy as np
from pathlib import Path


def imread(p):
    return cv2.imdecode(np.fromfile(str(p), dtype=np.uint8), cv2.IMREAD_UNCHANGED)


def imwrite(p, img, ext=".png"):
    ok, buf = cv2.imencode(ext, img)
    if not ok:
        raise IOError(f"编码失败 {p}")
    Path(p).write_bytes(buf.tobytes())


def main():
    ap = argparse.ArgumentParser(description="视频速诊：拼图 + 背景色 + 运动时间线")
    ap.add_argument("--video", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cols", type=int, default=6, help="拼图列数")
    ap.add_argument("--rows", type=int, default=5, help="拼图行数")
    args = ap.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    cap = cv2.VideoCapture(str(args.video))
    n = int(cap.get(cv2.CAP_PROP_FRAME_COUNT))
    fps = cap.get(cv2.CAP_PROP_FPS)
    print(f"[视频] {n} 帧 @ {fps:.1f}fps = {n/fps:.2f}s")

    frames = []
    while True:
        ok, f = cap.read()
        if not ok:
            break
        frames.append(f)
    cap.release()
    n = len(frames)
    h, w = frames[0].shape[:2]
    print(f"[实际] 读到 {n} 帧，尺寸 {w}x{h}")

    # ---- 1. 实测背景色：取四角区域（主体一般居中） ----
    corner = np.vstack([
        frames[n // 2][:40, :40].reshape(-1, 3),
        frames[n // 2][:40, -40:].reshape(-1, 3),
        frames[n // 2][-40:, :40].reshape(-1, 3),
        frames[n // 2][-40:, -40:].reshape(-1, 3),
    ]).astype(np.float64)
    bg_bgr = corner.mean(axis=0)
    bg_std = corner.std(axis=0)
    print(f"\n[背景色] 四角实测 BGR = ({bg_bgr[0]:.1f}, {bg_bgr[1]:.1f}, {bg_bgr[2]:.1f})"
          f"  std = ({bg_std[0]:.1f}, {bg_std[1]:.1f}, {bg_std[2]:.1f})")
    print(f"         RGB = ({bg_bgr[2]:.1f}, {bg_bgr[1]:.1f}, {bg_bgr[0]:.1f})")
    print(f"         G - max(R,B) = {bg_bgr[1] - max(bg_bgr[0], bg_bgr[2]):.1f}"
          f"  （绿幕应显著为正，>60 为佳）")

    # ---- 2. 等间隔拼图 ----
    idx = np.linspace(0, n - 1, args.cols * args.rows).astype(int)
    tile_h, tile_w = h // 2, w // 2
    canvas = np.zeros((args.rows * tile_h, args.cols * tile_w, 3), np.uint8)
    canvas[:] = 40
    for k, i in enumerate(idx):
        r, c = divmod(k, args.cols)
        t = cv2.resize(frames[i], (tile_w, tile_h), interpolation=cv2.INTER_AREA)
        canvas[r * tile_h:(r + 1) * tile_h, c * tile_w:(c + 1) * tile_w] = t
        cv2.putText(canvas, f"f{i:04d}", (c * tile_w + 4, r * tile_h + 20),
                    cv2.FONT_HERSHEY_SIMPLEX, 0.55, (0, 255, 255), 2)
    imwrite(out / "01-时间线拼图.png", canvas)

    # ---- 3. 运动时间线（绿幕区域之外的像素变化） ----
    # 粗前景：离背景色距离 > 阈值的像素
    tl = []
    for i in range(1, n):
        d = np.abs(frames[i].astype(np.int16) - frames[i - 1].astype(np.int16)).mean()
        tl.append(float(d))
    tl = np.array(tl)
    print(f"\n[运动] 全画面帧间差 均值 {tl.mean():.3f}  峰值 {tl.max():.3f}  最小 {tl.min():.3f}")
    # 找连续活动段
    thr = tl.mean() + 1.5 * tl.std()
    act = np.nonzero(tl > thr)[0]
    segs = []
    if len(act):
        s = act[0]
        for a, b in zip(act, act[1:]):
            if b - a > 5:
                segs.append((int(s), int(a)))
                s = b
        segs.append((int(s), int(act[-1])))
    print(f"[运动] 活动段(>{thr:.2f}) {len(segs)} 段: {segs[:15]}")

    # ---- 4. 首尾帧差异可视化 ----
    diff = np.abs(frames[0].astype(np.int16) - frames[-1].astype(np.int16)).astype(np.uint8)
    imwrite(out / "02-首尾差异.png", diff)
    print(f"\n[首尾] 平均差 {diff.mean():.2f}  最大差 {diff.max()}")

    # ---- 5. 绿幕 spill 检查：主体边缘像素的 G 通道超出量 ----
    # 前景 = 离背景色远
    mid = frames[n // 2].astype(np.float64)
    dist = np.abs(mid - bg_bgr).sum(axis=2)
    fg = dist > 120
    fg_px = mid[fg]
    if len(fg_px):
        excess = fg_px[:, 1] - np.maximum(fg_px[:, 0], fg_px[:, 2])
        print(f"\n[Spill] 主体像素 G-max(R,B): 均值 {excess.mean():.1f}  "
              f"最大 {excess.max():.1f}  >18占比 {100*(excess > 18).mean():.2f}%")
        print(f"        主体占比 {100*fg.mean():.1f}%")
        print(f"        主体 BGR 均值 ({fg_px[:,0].mean():.0f}, {fg_px[:,1].mean():.0f}, {fg_px[:,2].mean():.0f})"
              f"  RGB ({fg_px[:,2].mean():.0f}, {fg_px[:,1].mean():.0f}, {fg_px[:,0].mean():.0f})")

    print(f"\n[输出] {out}")


if __name__ == "__main__":
    main()

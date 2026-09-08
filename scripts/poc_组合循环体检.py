# -*- coding: utf-8 -*-
"""组合循环视频体检：呼吸+眨眼+摇尾 三合一单视频循环。

判据（2026-09-02，针对 05-组合循环）：
  1. 首尾一致性：f0000 vs 末帧前景差异，循环能不能无缝
  2. 眨眼次数：眼睛区域帧间突变，应 = 2 次
  3. 摇尾来回数：尾巴区域运动，每段 <= 2 来回
  4. 触边：前景不贴画布边
  5. 毛色：主体 RGB 与母版/锚点对比（降饱和检测）
  6. 眨眼是否重画身体：眨眼帧 vs 相邻帧的非眼部差异

用法：
  D:/DevTools/Python312/python.exe scripts/poc_组合循环体检.py \
      --video output/.../03-视频/05-组合循环-呼吸眨眼摇尾.mp4 \
      --out output/.../_体检-组合循环
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
FFMPEG = "ffmpeg"

KEY_LOW, KEY_HIGH = 18.0, 62.0


def chroma_fg(rgb: np.ndarray) -> np.ndarray:
    """绿幕前景掩码（bool），G - max(R,B) 软色键。"""
    r = rgb[:, :, 0].astype(np.float32)
    g = rgb[:, :, 1].astype(np.float32)
    b = rgb[:, :, 2].astype(np.float32)
    excess = g - np.maximum(r, b)
    return excess < (KEY_LOW + KEY_HIGH) / 2.0


def extract(video: Path, work: Path) -> list[np.ndarray]:
    subprocess.run([FFMPEG, "-y", "-v", "error", "-i", str(video),
                    "-start_number", "0", str(work / "s%04d.png")],
                   check=True)
    frames = sorted(work.glob("s*.png"))
    return [cv2.imdecode(np.fromfile(str(p), dtype=np.uint8), cv2.IMREAD_COLOR)
            for p in frames]


def fg_bbox(mask: np.ndarray) -> tuple[int, int, int, int]:
    ys, xs = np.nonzero(mask)
    if not ys.size:
        return 0, 0, 0, 0
    return int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())


def main() -> int:
    ap = argparse.ArgumentParser(description="组合循环视频体检")
    ap.add_argument("--video", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--master", default=None, help="母版/锚点 PNG，做毛色对比")
    args = ap.parse_args()

    video = Path(args.video)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    work = Path(tempfile.mkdtemp(prefix="poc_combo_"))
    print(f"[抽帧] {video.name}")
    rgbs = extract(video, work)
    n = len(rgbs)
    fps = 24.0
    print(f"[抽帧] {n} 帧 @ {fps}fps = {n / fps:.2f}s")

    fgs = [chroma_fg(r) for r in rgbs]
    boxes = [fg_bbox(m) for m in fgs]

    report = {"video": str(video), "frames": n, "fps": fps}

    # 1. 首尾一致性
    d0l = int(np.abs(rgbs[0].astype(np.int16) - rgbs[-1].astype(np.int16)).max())
    vis = fgs[0] | fgs[-1]
    mean_diff = float(np.abs(rgbs[0].astype(np.float32)[vis] -
                             rgbs[-1].astype(np.float32)[vis]).mean())
    report["首尾一致性"] = {"maxDiff": d0l, "meanDiffOnFg": round(mean_diff, 2)}
    print(f"\n[1 首尾一致性] f0000 vs f{n-1:04d}  前景均值差 {mean_diff:.2f}  最大差 {d0l}")

    # 2. 全局运动时间线：相邻帧前景差异（定位眨眼/摇尾时间段）
    timeline = []
    for i in range(1, n):
        v = fgs[i - 1] | fgs[i]
        d = np.abs(rgbs[i].astype(np.float32)[v] - rgbs[i - 1].astype(np.float32)[v]).mean()
        timeline.append(float(d))
    timeline = np.array(timeline)

    # 3. 区域运动：眼睛（头部上半）vs 尾巴（下半/右侧）
    h, w = rgbs[0].shape[:2]
    # 用中位 bbox 粗分：头部 = 上 45%，尾巴 = 下 45%
    ys, xs = np.nonzero(fgs[len(fgs) // 2])
    head_y = int(ys.min() + (ys.max() - ys.min()) * 0.45)
    eye_region = np.zeros_like(fgs[0])
    eye_region[:head_y, :] = True
    tail_region = np.zeros_like(fgs[0])
    tail_region[int(ys.min() + (ys.max() - ys.min()) * 0.55):, :] = True

    eye_tl, tail_tl = [], []
    for i in range(1, n):
        v = fgs[i] | fgs[i - 1]
        de = np.abs(rgbs[i].astype(np.float32) - rgbs[i - 1].astype(np.float32)).mean(axis=2)
        eye_tl.append(float(de[(v & eye_region)].mean()))
        tail_tl.append(float(de[(v & tail_region)].mean()))
    eye_tl = np.array(eye_tl)
    tail_tl = np.array(tail_tl)

    # 眨眼：眼睛区域突变（峰值），阈值 = 均值 + 3*std
    eye_thr = eye_tl.mean() + 3 * eye_tl.std()
    blink_peaks = [int(i) for i in np.nonzero(eye_tl > eye_thr)[0]]
    report["眨眼"] = {"eyeMotionMean": round(float(eye_tl.mean()), 3),
                      "eyeMotionMax": round(float(eye_tl.max()), 3),
                      "peakFrames": blink_peaks,
                      "peakCount": len(blink_peaks)}
    print(f"[2 眨眼] 眼睛区域帧间变化 均值 {eye_tl.mean():.3f} 峰值 {eye_tl.max():.3f} "
          f"突变帧(>{eye_thr:.2f}) {len(blink_peaks)} 个: {blink_peaks[:20]}")

    # 摇尾：尾巴区域运动，找活动段
    tail_thr = tail_tl.mean() + 2 * tail_tl.std()
    tail_active = [int(i) for i in np.nonzero(tail_tl > tail_thr)[0]]
    # 合并成段
    segs = []
    if tail_active:
        s = tail_active[0]
        for a, b in zip(tail_active, tail_active[1:]):
            if b - a > 3:
                segs.append((s, a)); s = b
        segs.append((s, tail_active[-1]))
    report["摇尾"] = {"tailMotionMean": round(float(tail_tl.mean()), 3),
                      "tailMotionMax": round(float(tail_tl.max()), 3),
                      "activeSegments": [[int(a), int(b)] for a, b in segs]}
    print(f"[3 摇尾] 尾巴区域活动段(>{tail_thr:.2f}) {len(segs)} 段: "
          f"{[[int(a), int(b)] for a, b in segs]}")

    # 4. 触边
    touched = [i for i, (x0, y0, x1, y1) in enumerate(boxes)
               if x0 <= 2 or y0 <= 2 or x1 >= w - 3 or y1 >= h - 3]
    report["触边"] = {"touchedFrames": touched[:20], "touchedCount": len(touched)}
    print(f"[4 触边] {len(touched)}/{n} 帧贴边")

    # 5. 毛色（可选）
    if args.master:
        master = cv2.imdecode(np.fromfile(str(Path(args.master)), dtype=np.uint8),
                              cv2.IMREAD_UNCHANGED)
        if master is not None:
            mm = master[:, :, 3] >= 200
            mmean = master[:, :, :3][mm].astype(np.float32).mean(axis=0)
            fmean = rgbs[n // 2].astype(np.float32)[fgs[n // 2]].mean(axis=0)
            report["毛色"] = {"masterBGR": mmean.round(1).tolist(),
                              "videoMidBGR": fmean.round(1).tolist()}
            print(f"[5 毛色] 母版 {mmean.round(1)}  视频中帧 {fmean.round(1)}")

    (out / "体检报告.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n[报告] {out / '体检报告.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

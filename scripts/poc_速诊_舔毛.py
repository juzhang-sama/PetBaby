# -*- coding: utf-8 -*-
"""舔毛视频速诊：判归属 + 量取景 + 查 f0000 镜头切换 + 测绿幕色。

只做只读诊断，不产出资产。判定依据：
1. 视频中后段帧的主体 RGB vs 两只猫 idle-combo 的锚点帧
2. 非绿 bbox 位置 vs 两只猫的 crop box 坐标系
"""
from __future__ import annotations

import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
PETS = {
    "04-warm-brown-tabby": "毛砌墙（暖棕虎斑）",
    "05-silver-tabby": "建国（银渐层）",
}


def read_anchor(pet_id: str) -> np.ndarray:
    p = ROOT / "apps/desktop/public/builtin-pets" / pet_id / "frames/idle-combo/f0000.png"
    return cv2.imdecode(np.fromfile(str(p), np.uint8), cv2.IMREAD_UNCHANGED)


def nongreen_mask(bgr: np.ndarray) -> np.ndarray:
    b, g, r = bgr[:, :, 0].astype(int), bgr[:, :, 1].astype(int), bgr[:, :, 2].astype(int)
    return (g - np.maximum(r, b)) < 40


def circular_hue_mean(hsv: np.ndarray) -> float:
    """OpenCV 色相 0..179 == 0..358°，是角度，必须圆均值。"""
    h_rad = np.radians(hsv[:, 0].astype(float) * 2.0)
    return float(np.degrees(np.arctan2(np.sin(h_rad).mean(), np.cos(h_rad).mean())) % 360) / 2.0


def body_stats(bgra: np.ndarray, label: str) -> dict:
    a = bgra[:, :, 3]
    m = (a > 200) & nongreen_mask(bgra[:, :, :3])
    if m.sum() < 500:
        return {"label": label, "n": int(m.sum())}
    b, g, r = (bgra[:, :, 0].astype(int), bgra[:, :, 1].astype(int), bgra[:, :, 2].astype(int))
    rgb = np.stack([r[m], g[m], b[m]], 1)
    hsv = cv2.cvtColor(rgb.reshape(-1, 1, 3).astype(np.uint8), cv2.COLOR_RGB2HSV).reshape(-1, 3)
    return {
        "label": label,
        "n": int(m.sum()),
        "rgb_mean": [round(float(v), 1) for v in rgb.mean(0)],
        "sat_median": float(np.median(hsv[:, 1])),
        "hue": round(circular_hue_mean(hsv), 1),
    }


def frame_report(bgr: np.ndarray) -> dict:
    """绿幕原帧：非绿占比 + bbox + 主体 RGB + 饱和。"""
    m = nongreen_mask(bgr)
    ys, xs = np.nonzero(m)
    b, g, r = bgr[:, :, 0].astype(int), bgr[:, :, 1].astype(int), bgr[:, :, 2].astype(int)
    rgb = np.stack([r[m], g[m], b[m]], 1)
    hsv = cv2.cvtColor(rgb.reshape(-1, 1, 3).astype(np.uint8), cv2.COLOR_RGB2HSV).reshape(-1, 3)
    return {
        "ratio": float(m.mean()),
        "bbox": (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())),
        "rgb": rgb.mean(0),
        "sat": float(np.median(hsv[:, 1])),
        "hue": round(circular_hue_mean(hsv), 1),
    }


def main() -> int:
    videos = {
        "A-788446821521": "C:/Users/Administrator/Downloads/seedance-2-0-official-1788446821521.mp4",
        "B-788446989221": "C:/Users/Administrator/Downloads/seedance-2-0-official-1788446989221.mp4",
    }

    print("=== 参照：两只猫 idle-combo 锚点帧 ===")
    anchors = {}
    for pid, name in PETS.items():
        s = body_stats(read_anchor(pid), name)
        anchors[pid] = s
        print(f"  {pid:24s} RGB={s['rgb_mean']}  饱和中位={s['sat_median']:.0f}  "
              f"色相={s['hue']:.1f}°  像素={s['n']}")

    print()
    for tag, path in videos.items():
        cap = cv2.VideoCapture(path)
        if not cap.isOpened():
            print(f"[{tag}] 打不开 {path}")
            return 1
        n = int(cap.get(cv2.CAP_PROP_FRAME_COUNT))
        fps = cap.get(cv2.CAP_PROP_FPS)
        w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
        h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
        print(f"=== 视频 {tag} ===  {w}x{h}  {fps:.2f}fps  {n}帧  {n/fps:.2f}s")

        picks = [0, n // 3, (2 * n) // 3, n - 1]
        frames = {}
        for i in picks:
            cap.set(cv2.CAP_PROP_POS_FRAMES, i)
            ok, fr = cap.read()
            if ok:
                frames[i] = fr
        cap.release()

        # 绿幕背景色：取四角块（被主体污染的概率最低）
        f0 = frames[picks[0]]
        corner = np.vstack([
            f0[:40, :40].reshape(-1, 3), f0[:40, -40:].reshape(-1, 3),
            f0[-40:, :40].reshape(-1, 3), f0[-40:, -40:].reshape(-1, 3),
        ]).astype(int)
        cb, cg, cr = corner[:, 0], corner[:, 1], corner[:, 2]
        print(f"  [绿幕] 角块 RGB=({cr.mean():.0f},{cg.mean():.0f},{cb.mean():.0f})  "
              f"G-max(R,B)={(cg - np.maximum(cr, cb)).mean():.1f}")

        reports = {}
        for i in picks:
            if i not in frames:
                continue
            rep = frame_report(frames[i])
            reports[i] = rep
            print(f"  f{i:05d}: 非绿占比={rep['ratio']*100:5.1f}%  bbox={rep['bbox']}  "
                  f"RGB=[{rep['rgb'][0]:.0f},{rep['rgb'][1]:.0f},{rep['rgb'][2]:.0f}]  "
                  f"饱和中位={rep['sat']:.0f}  色相={rep['hue']:.1f}°")

        # 归属判定：用中后段帧（避开 f0 可能的镜头切换）
        cur = reports[picks[2]]["rgb"]
        print("  [归属] RGB 距离：", end="")
        best, bestd = None, 1e9
        for pid in PETS:
            d = float(np.linalg.norm(cur - np.array(anchors[pid]["rgb_mean"])))
            print(f"  {PETS[pid]}={d:.1f}", end="")
            if d < bestd:
                best, bestd = pid, d
        print(f"\n  → 判定：{best} ({PETS[best]})，距离 {bestd:.1f}")

        # 镜头切换检查
        r0, r1 = reports[picks[0]]["ratio"], reports[picks[2]]["ratio"]
        print(f"  [镜头检查] f0000 占比={r0*100:.1f}% bbox={reports[picks[0]]['bbox']}  |  "
              f"f{picks[2]:05d} 占比={r1*100:.1f}% bbox={reports[picks[2]]['bbox']}")
        if abs(r0 - r1) > 0.15:
            print("  ⚠️  构图占比差异 >15%，可能有镜头切换，f0000 不可信")
        else:
            print("  ✓  构图占比接近，无镜头切换迹象")
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())

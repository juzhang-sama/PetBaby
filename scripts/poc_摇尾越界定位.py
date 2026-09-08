# -*- coding: utf-8 -*-
"""摇尾巴体检 FAIL 项定位：触边方向/位置 + 尾段收敛超标的原因。"""
import cv2
import numpy as np
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from poc_抠像 import chroma_alpha, clean_mask, spatial_smooth  # noqa: E402

src = Path(__file__).parent / "poc_动作单元体检.py"
src_text = src.read_text(encoding="utf-8")

m = re.search(r"TOUCH_MARGIN\s*=\s*(\d+)", src_text)
TOUCH_MARGIN = int(m.group(1))
m = re.search(r"TAIL_WINDOW\s*=\s*(\d+)", src_text)
TAIL_WINDOW = int(m.group(1))

VIDEO = "output/宠物动作-毛砌墙-v3-2026-08-31/03-视频/02-摇尾巴.mp4"
CROP_X, CROP_Y, CROP_S = 0, 34, 882
OUT_SIZE = 588

cap = cv2.VideoCapture(VIDEO)
alphas = []
while True:
    ok, bgr = cap.read()
    if not ok:
        break
    rgb = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
    a = 1.0 - chroma_alpha(rgb)
    a = clean_mask(a)
    a = spatial_smooth(a, 0.6)
    alphas.append(np.clip(a, 0.0, 1.0))
cap.release()
print(f"帧数: {len(alphas)}")

# 裁剪 + resize
crops = []
for a in alphas:
    c = a[CROP_Y:CROP_Y + CROP_S, CROP_X:CROP_X + CROP_S]
    c = cv2.resize(c, (OUT_SIZE, OUT_SIZE), interpolation=cv2.INTER_AREA)
    crops.append(c)
masks = [c >= 0.5 for c in crops]

print(f"\n=== 触边定位（TOUCH_MARGIN={TOUCH_MARGIN}）===")
for i, m_ in enumerate(masks):
    sides = []
    if m_[:, :TOUCH_MARGIN].any():
        sides.append("左")
    if m_[:, -TOUCH_MARGIN:].any():
        sides.append("右")
    if m_[:TOUCH_MARGIN, :].any():
        sides.append("上")
    if m_[-TOUCH_MARGIN:, :].any():
        sides.append("下")
    if sides:
        detail = ""
        if "右" in sides:
            rows = np.where(m_[:, -TOUCH_MARGIN:].any(axis=1))[0]
            detail += f" 右边行{rows.min()}~{rows.max()}"
        if "下" in sides:
            cols = np.where(m_[-TOUCH_MARGIN:, :].any(axis=0))[0]
            detail += f" 下边列{cols.min()}~{cols.max()}"
        if "上" in sides:
            cols = np.where(m_[:TOUCH_MARGIN, :].any(axis=0))[0]
            detail += f" 上边列{cols.min()}~{cols.max()}"
        if "左" in sides:
            rows = np.where(m_[:, :TOUCH_MARGIN].any(axis=1))[0]
            detail += f" 左边行{rows.min()}~{rows.max()}"
        print(f"帧{i:3d}: 触{'+'.join(sides)}边{detail}")

print(f"\n=== 尾段收敛定位（最后 {TAIL_WINDOW} 帧）===")
k = TAIL_WINDOW
for i in range(len(crops) - k, len(crops) - 1):
    d = np.abs(crops[i] - crops[i + 1])
    moved = d > 0.1
    n = int(moved.sum())
    if n > 0:
        ys, xs = np.where(moved)
        print(f"帧{i:3d}->{i+1:3d}: 变化像素 {n:5d}  x范围{xs.min()}~{xs.max()}  "
              f"y范围{ys.min()}~{ys.max()}")

print("\n=== 峰值帧运动分布 ===")
diffs = [np.abs(crops[i] - crops[i + 1]).sum() for i in range(len(crops) - 1)]
peak_i = int(np.argmax(diffs))
print(f"峰值帧对: {peak_i}->{peak_i+1}  差分总量 {diffs[peak_i]:.0f}")
d = np.abs(crops[peak_i] - crops[peak_i + 1]) > 0.1
ys, xs = np.where(d)
print(f"  变化像素 {int(d.sum())}  x范围{xs.min()}~{xs.max()}  y范围{ys.min()}~{ys.max()}")

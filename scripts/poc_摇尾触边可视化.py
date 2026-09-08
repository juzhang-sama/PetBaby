# -*- coding: utf-8 -*-
"""触边帧视觉影响评估：裁剪后 588 画布上尾巴被切的程度。"""
import cv2
import numpy as np
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from poc_抠像 import chroma_alpha, clean_mask, spatial_smooth  # noqa: E402

VIDEO = "output/宠物动作-毛砌墙-v3-2026-08-31/03-视频/02-摇尾巴.mp4"
OUT = "output/宠物动作-毛砌墙-v3-2026-08-31/_体检-摇尾巴-v1/触边帧-视觉影响.png"

CROP_X, CROP_Y, CROP_S = 0, 34, 882

cap = cv2.VideoCapture(VIDEO)
frames = []
while True:
    ok, bgr = cap.read()
    if not ok:
        break
    frames.append(bgr)
cap.release()

picks = [0, 49, 98, 100, 102, 104]
cells = []
for i in picks:
    crop = frames[i][CROP_Y:CROP_Y + CROP_S, CROP_X:CROP_X + CROP_S]
    small = cv2.resize(crop, (588, 588), interpolation=cv2.INTER_AREA)
    rgb = cv2.cvtColor(small, cv2.COLOR_BGR2RGB)
    a = 1.0 - chroma_alpha(rgb)
    a = clean_mask(a)
    a = spatial_smooth(a, 0.6)
    a = np.clip(a, 0.0, 1.0)
    comp = (rgb * a[..., None]).astype(np.uint8)  # 黑底合成
    comp[:, -3:] = [255, 60, 60]                  # 红线=画布右边界
    cells.append(comp)

row = np.hstack(cells)

# 帧 100 尾巴区域 4x 放大（右下角）
f100 = cells[3]
tail = f100[380:588, 380:588]
tail_big = cv2.resize(tail, (588, 588), interpolation=cv2.INTER_NEAREST)
pad = np.zeros((588, 588 * 6 - tail_big.shape[1], 3), np.uint8)
bottom = np.hstack([tail_big, pad])

out = np.vstack([row, bottom])
cv2.imencode(".png", cv2.cvtColor(out, cv2.COLOR_RGB2BGR))[1].tofile(OUT)
print(f"已输出 {OUT}")
print("上排: 帧 0(锚点) 49 98 100 102 104 → 裁剪后 588 画布（右侧红线=画布边界）")
print("下排: 帧 100 尾巴区域 4x 放大")

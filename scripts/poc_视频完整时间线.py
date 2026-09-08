# -*- coding: utf-8 -*-
"""完整时间线拼图：每 12 帧抽 1 张，6 列 × 5 行 = 30 张覆盖 12 秒"""
import cv2
import numpy as np
from pathlib import Path

v = Path('output/宠物动作-建国-v1-2026-09-03/03-视频/组合循环-原始.mp4')
out = Path('output/宠物动作-建国-v1-2026-09-03/04-体检/速诊')

cap = cv2.VideoCapture(str(v))
frames = []
while True:
    ok, f = cap.read()
    if not ok:
        break
    frames.append(f)
cap.release()
n = len(frames)
h, w = frames[0].shape[:2]

cols, rows = 6, 5
total = cols * rows
idx = np.linspace(0, n - 1, total).astype(int)
tile_h, tile_w = h // 2, w // 2
canvas = np.zeros((rows * tile_h, cols * tile_w, 3), np.uint8)
canvas[:] = 30
for k, i in enumerate(idx):
    r, c = divmod(k, cols)
    t = cv2.resize(frames[i], (tile_w, tile_h), interpolation=cv2.INTER_AREA)
    canvas[r * tile_h:(r + 1) * tile_h, c * tile_w:(c + 1) * tile_w] = t
    cv2.putText(canvas, f"f{i:03d}", (c * tile_w + 4, r * tile_h + 18),
                cv2.FONT_HERSHEY_SIMPLEX, 0.6, (0, 255, 255), 2)

_, buf = cv2.imencode('.png', canvas)
(out / '00-完整时间线.png').write_bytes(buf.tobytes())
print(f'已保存 {out}/00-完整时间线.png ({total} 帧覆盖 {n} 帧原始)')
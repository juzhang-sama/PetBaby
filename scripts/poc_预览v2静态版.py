#!/usr/bin/env python3
"""模拟 v2 预览页渲染：5 帧（呼吸 + 眨眼 4 帧）x 3 种背景的对比大图。

为什么需要：v1 预览页有 bug（默认间隔 7.56s + 眨眼仅 168ms），老王几乎看不到眨眼。
v2 修了页面 + 加了眼周放大窗口。本脚本用 Python 复现 v2 页面布局，输出静态对比图，
老王不用打开浏览器就能一眼看清：主画 320（变化像素只占 2.12% 几乎看不出区别）
+ 眼周 4× 放大（眼睛闭合清晰可见）+ 三种背景（看接缝/残留）。
"""

from __future__ import annotations
import numpy as np
import cv2
from pathlib import Path

ROOT = Path(r"D:/petBaby/desktop-pet/output/宠物动作-毛砌墙-v3-2026-08-31")
D06 = ROOT / "06-眨眼"


def rd(p: Path) -> np.ndarray:
    return cv2.imdecode(np.fromfile(str(p), np.uint8), cv2.IMREAD_UNCHANGED)


def composite(rgba: np.ndarray, bg_img: np.ndarray) -> np.ndarray:
    """按 alpha 把 RGBA 合成到 bg_img 上。bg_img 已是目标尺寸的 3 通道图。"""
    if rgba.shape[2] == 3:
        rgba = np.dstack([rgba, np.full(rgba.shape[:2], 255, np.uint8)])
    a = rgba[:, :, 3:4].astype(np.float32) / 255.0
    return (rgba[:, :, :3].astype(np.float32) * a
            + bg_img.astype(np.float32) * (1 - a)).astype(np.uint8)


def to_main(rgba: np.ndarray, bg: np.ndarray) -> np.ndarray:
    """缩放到 320x320 合成到 bg（bg 已经是 320x320x3 的图）。"""
    out = cv2.resize(rgba, (320, 320), interpolation=cv2.INTER_NEAREST)
    return composite(out, bg)


def to_eye(rgba: np.ndarray, bg: np.ndarray, eye_bbox: dict) -> np.ndarray:
    """裁眼周 bbox，放大 4 倍合成到 bg。bg 任意尺寸，内部 resize 对齐。"""
    H, W = rgba.shape[:2]
    x0 = int(eye_bbox["x"] * W)
    y0 = int(eye_bbox["y"] * H)
    x1 = x0 + int(eye_bbox["w"] * W)
    y1 = y0 + int(eye_bbox["h"] * H)
    crop = rgba[y0:y1, x0:x1]
    EYE_W = 280
    EYE_H = round(EYE_W * eye_bbox["h"] / eye_bbox["w"])
    out = cv2.resize(crop, (EYE_W, EYE_H), interpolation=cv2.INTER_NEAREST)
    if bg.shape[:2] != (EYE_H, EYE_W):
        bg = cv2.resize(bg, (EYE_W, EYE_H), interpolation=cv2.INTER_NEAREST)
    return composite(out, bg)


def checker_bg(size: int) -> np.ndarray:
    cell = max(4, size // 24)
    c = np.zeros((size, size, 3), np.float32)
    for y in range(0, size, cell):
        for x in range(0, size, cell):
            v = 230.0 if ((x // cell + y // cell) % 2 == 0) else 200.0
            c[y:y + cell, x:x + cell] = [v, v, v]
    return c


def main() -> None:
    frames = [
        ("呼吸 锚点", rd(ROOT / "04-帧序列-呼吸循环/frames/f0000.png")),
        ("眨眼 f0 强度0.35", rd(D06 / "01-帧序列/frames/f0000.png")),
        ("眨眼 f1 强度1.0", rd(D06 / "01-帧序列/frames/f0001.png")),
        ("眨眼 f2 强度1.0", rd(D06 / "01-帧序列/frames/f0002.png")),
        ("眨眼 f3 强度0.45", rd(D06 / "01-帧序列/frames/f0003.png")),
    ]
    EYE = dict(x=0.1514, y=0.1939, w=0.2211, h=0.1173)
    EYE_W = 280
    EYE_H = round(EYE_W * EYE["h"] / EYE["w"])

    BGS = [
        ("黑底（默认）", np.full((320, 320, 3), [20, 20, 20], np.float32),
         np.full((EYE_H, EYE_W, 3), [20, 20, 20], np.float32)),
        ("白底（看接缝）", np.full((320, 320, 3), [240, 240, 240], np.float32),
         np.full((EYE_H, EYE_W, 3), [240, 240, 240], np.float32)),
        ("棋盘格（看残留）", checker_bg(320), checker_bg(EYE_H)),
    ]

    MAIN_S = 320
    cell_w = MAIN_S + EYE_W + 40
    cell_h = max(MAIN_S, EYE_H) + 60
    W = 40 + len(BGS) * cell_w
    H = 80 + len(frames) * cell_h
    canvas = np.full((H, W, 3), 24, np.uint8)

    cv2.putText(canvas, "v2 预览模拟  5 帧 x 3 背景  每格：主画 320 + 眼周 4x 放大",
                (20, 36), cv2.FONT_HERSHEY_SIMPLEX, 0.7, (220, 220, 220), 1, cv2.LINE_AA)

    for ci, (col_label, bg_main, bg_eye) in enumerate(BGS):
        x0 = 40 + ci * cell_w
        cv2.putText(canvas, col_label, (x0, 64), cv2.FONT_HERSHEY_SIMPLEX, 0.55,
                    (160, 180, 200), 1, cv2.LINE_AA)
        for ri, (name, frm) in enumerate(frames):
            y0 = 80 + ri * cell_h
            canvas[y0:y0 + MAIN_S, x0:x0 + MAIN_S] = to_main(frm, bg_main)
            ex = x0 + MAIN_S + 20
            canvas[y0:y0 + EYE_H, ex:ex + EYE_W] = to_eye(frm, bg_eye, EYE)
            cv2.rectangle(canvas, (ex, y0), (ex + EYE_W - 1, y0 + EYE_H - 1),
                          (90, 150, 255), 1)
            cv2.putText(canvas, name, (x0, y0 + MAIN_S + 28),
                        cv2.FONT_HERSHEY_SIMPLEX, 0.5, (200, 200, 200), 1, cv2.LINE_AA)

    out = D06 / "v2-预览模拟.png"
    cv2.imencode(".png", canvas)[1].tofile(str(out))
    print(f"输出: {out}  size={canvas.shape[1]}x{canvas.shape[0]}  "
          f"({canvas.nbytes // 1024} KB)")


if __name__ == "__main__":
    main()

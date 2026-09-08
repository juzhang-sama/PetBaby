# -*- coding: utf-8 -*-
"""定位 wipe 模式 p=0 帧仍然变化的像素在哪（相对于眼球 bbox）。

背景：wipe 模式首末帧 p=0，理论上遮罩应处处为 0、输出与锚点帧逐像素相同，
但实测有 121 个像素变了、最大差 193。这个脚本回答"这 121 个像素是谁"：
  1. 落在眼球 bbox 内 / 上方（额头）/ 下方（脸颊）/ 左右（眼角外）
  2. 反解出的混合权重 m 是多少（0.25？0.5？1.0？）
  3. 出一张放大图，肉眼直接看是不是"额头皮纹差异"

用法：
  D:/DevTools/Python312/python.exe scripts/poc_眨眼泄漏定位.py \
      --anchor output/.../04-帧序列-呼吸循环/frames/f0000.png \
      --frames output/.../06-眨眼/04-wipe-8帧-336ms/frames \
      --index 0
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))


def imread_u(path: Path) -> np.ndarray:
    img = cv2.imdecode(np.fromfile(str(path), dtype=np.uint8), cv2.IMREAD_UNCHANGED)
    if img is None:
        raise SystemExit(f"[读图失败] {path}")
    return img


def imwrite_u(path: Path, img: np.ndarray) -> None:
    ok, buf = cv2.imencode(".png", img)
    if not ok:
        raise SystemExit(f"[写图失败] {path}")
    buf.tofile(str(path))
    print(f"[输出] {path}")


def detect_eyes(anchor: np.ndarray):
    a = anchor[:, :, 3]
    rgb = anchor[:, :, :3]
    hsv = cv2.cvtColor(rgb, cv2.COLOR_BGR2HSV)
    h = hsv[:, :, 0].astype(np.int16)
    s = hsv[:, :, 1].astype(np.int16)
    v = hsv[:, :, 2].astype(np.int16)
    mask = ((h >= 18) & (h <= 45) & (s >= 100) & (v >= 120) & (a > 128)).astype(np.uint8) * 255
    n, _labels, stats, _cent = cv2.connectedComponentsWithStats(mask, 8)
    if n < 2:
        raise SystemExit("[错误] 没检测到两只眼睛")
    comps = sorted(
        ((int(stats[i][4]), int(stats[i][1]), int(stats[i][0]),
          int(stats[i][3]), int(stats[i][2])) for i in range(1, n)),
        reverse=True,
    )[:2]
    eyes = [(int(c[1]), int(c[1] + c[3]), int(c[2]), int(c[2] + c[4])) for c in comps]
    eyes.sort(key=lambda e: e[2])
    ys0 = min(e[0] for e in eyes)
    ys1 = max(e[1] for e in eyes)
    xs0 = min(e[2] for e in eyes)
    xs1 = max(e[3] for e in eyes)
    return (ys0, ys1, xs0, xs1), eyes


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--anchor", required=True)
    ap.add_argument("--frames", required=True, help="wipe 帧目录")
    ap.add_argument("--index", type=int, default=0, help="检查第几帧（p=0 的那帧）")
    ap.add_argument("--eye-pad", type=int, default=16)
    ap.add_argument("--zoom", type=int, default=8)
    args = ap.parse_args()

    anchor = imread_u(Path(args.anchor))
    fdir = Path(args.frames)
    imgs = sorted(fdir.glob("f*.png"))
    if not imgs:
        raise SystemExit(f"[错误] {fdir} 里没有帧")
    frame = imread_u(imgs[args.index])

    (ey0, ey1, ex0, ex1), eyes = detect_eyes(anchor)
    print(f"[眼睛] 左 y[{eyes[0][0]},{eyes[0][1]}] x[{eyes[0][2]},{eyes[0][3]}]  "
          f"右 y[{eyes[1][0]},{eyes[1][1]}] x[{eyes[1][2]},{eyes[1][3]}]")
    print(f"[眼睛] 合并 y[{ey0},{ey1}] x[{ex0},{ex1}]")

    d = np.abs(frame.astype(np.float32) - anchor.astype(np.float32)).max(axis=2)
    changed = d > 8
    print(f"\n[变化] {imgs[args.index].name} 变化像素 {int(changed.sum())}  最大差 {d.max():.1f}")

    if not changed.any():
        print("  无变化，首末帧与锚点完全一致 ✅")
        return 0

    # 分类：相对每只眼睛的 bbox
    cy, cx = np.nonzero(changed)
    cats = {"眼球内": 0, "上方(额头)": 0, "下方(脸颊)": 0, "左右(眼角外)": 0}
    rows: dict[str, list] = {"眼球内": [], "上方(额头)": [], "下方(脸颊)": []}
    for y, x in zip(cy, cx):
        placed = False
        for (a0, a1, b0, b1) in eyes:
            if b0 <= x < b1:
                if a0 <= y < a1:
                    cats["眼球内"] += 1
                    rows["眼球内"].append(y)
                elif y < a0:
                    cats["上方(额头)"] += 1
                    rows["上方(额头)"].append(y)
                else:
                    cats["下方(脸颊)"] += 1
                    rows["下方(脸颊)"].append(y)
                placed = True
                break
        if not placed:
            cats["左右(眼角外)"] += 1
    for k, v in cats.items():
        extra = ""
        if rows.get(k):
            arr = np.array(rows[k])
            extra = f"  y 范围 [{arr.min()},{arr.max()}]"
        print(f"  {k:14s} {v:5d}{extra}")

    # 反解这些像素上的等效混合权重（用同 patch 的闭眼帧当基准）
    full = imread_u(imgs[len(imgs) // 2])
    denom = np.abs(full.astype(np.float32) - anchor.astype(np.float32)).max(axis=2)
    valid = changed & (denom >= 24.0)
    if valid.any():
        num = np.abs(frame.astype(np.float32) - anchor.astype(np.float32)).max(axis=2)
        m = num[valid] / denom[valid]
        print(f"\n[等效 m] 均值 {m.mean():.3f}  中位 {np.median(m):.3f}  "
              f"范围 [{m.min():.3f},{m.max():.3f}]  (可反解像素 {int(valid.sum())})")

    # 出放大对比图：锚点 / 该帧 / 差值
    y0 = max(0, ey0 - args.eye_pad)
    y1 = min(anchor.shape[0], ey1 + args.eye_pad)
    x0 = max(0, ex0 - args.eye_pad)
    x1 = min(anchor.shape[1], ex1 + args.eye_pad)
    z = args.zoom

    def zoom_rgb(img, mark_eyes=False):
        crop = img[y0:y1, x0:x1]
        nz = cv2.resize(crop, ((x1 - x0) * z, (y1 - y0) * z), interpolation=cv2.INTER_NEAREST)
        if mark_eyes:
            for (a0, a1, b0, b1) in eyes:
                cv2.rectangle(nz, ((b0 - x0) * z, (a0 - y0) * z),
                              ((b1 - x0) * z - 1, (a1 - y0) * z - 1), (0, 0, 255, 255), 2)
        return nz

    bg = np.zeros((y1 - y0, x1 - x0, 3), np.uint8)

    def on_bg(img):
        c = img[y0:y1, x0:x1]
        out = bg.copy()
        al = (c[:, :, 3:4] / 255.0).astype(np.float32)
        out[:] = (c[:, :, :3] * al + np.array([24, 24, 24], np.float32) * (1 - al)).astype(np.uint8)
        return out

    a_z = cv2.resize(on_bg(anchor), ((x1 - x0) * z, (y1 - y0) * z), interpolation=cv2.INTER_NEAREST)
    f_z = cv2.resize(on_bg(frame), ((x1 - x0) * z, (y1 - y0) * z), interpolation=cv2.INTER_NEAREST)
    diff = np.clip(np.abs(frame.astype(np.float32) - anchor.astype(np.float32)).max(axis=2), 0, 255)
    d_z = cv2.resize(diff[y0:y1, x0:x1].astype(np.uint8),
                     ((x1 - x0) * z, (y1 - y0) * z), interpolation=cv2.INTER_NEAREST)
    d_z = cv2.applyColorMap(d_z, cv2.COLORMAP_JET)
    a_z = zoom_rgb(anchor.astype(np.uint8), False)
    a_z = cv2.resize(on_bg(anchor), ((x1 - x0) * z, (y1 - y0) * z), interpolation=cv2.INTER_NEAREST)
    for (ea0, ea1, eb0, eb1) in eyes:
        cv2.rectangle(a_z, ((eb0 - x0) * z, (ea0 - y0) * z),
                      ((eb1 - x0) * z - 1, (ea1 - y0) * z - 1), (0, 0, 255), 2)
    tile = np.hstack([a_z, f_z, d_z])
    outp = fdir.parent / f"_诊断-泄漏-{imgs[args.index].stem}.png"
    imwrite_u(outp, tile)
    print(f"\n[说明] 左=锚点(红框=虹膜bbox)  中={imgs[args.index].name}  右=差值热力图")
    return 0


if __name__ == "__main__":
    sys.exit(main())

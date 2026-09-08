# -*- coding: utf-8 -*-
"""摇尾巴帧序列后处理：选段 + 呼吸同框裁剪 + 首尾锚点替换。

输入：00-抠像原幅/frames（960×960 透明 PNG，warmup 已跳 3 帧）
输出：01-帧序列-XX帧（588×588，首尾 = 呼吸锚点帧，逐字节一致）

裁剪框必须与呼吸循环完全一致（归一化换算）：
  呼吸（640 画幅）x=0 y=23 size=588
  摇尾（960 画幅）x=0 y=34 size=882   (23*1.5=34.5→34, 588*1.5=882)
否则切换动作时猫会跳位。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_摇尾巴帧序列.py \
      --src   output/.../08-摇尾巴/00-抠像原幅/frames \
      --anchor output/.../04-帧序列-呼吸循环/frames/f0000.png \
      --start 7 --end 55 \
      --out   output/.../08-摇尾巴/01-帧序列
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]

# 960 画幅下的裁剪框（与呼吸循环 640 → x=0,y=23,size=588 归一化同框）
CROP_X, CROP_Y, CROP_S = 0, 34, 882
OUT_SIZE = 588
FRAME_MS = 42


def imread_rgba(path: Path) -> np.ndarray:
    """cv2.imread 不认中文路径，走 np.fromfile + imdecode。"""
    buf = np.fromfile(str(path), np.uint8)
    img = cv2.imdecode(buf, cv2.IMREAD_UNCHANGED)
    assert img is not None, f"图像读取失败: {path}"
    return img


def imwrite_rgba(path: Path, img: np.ndarray) -> None:
    """cv2.imwrite 同样不认中文路径，走 imencode + tofile。"""
    ok, buf = cv2.imencode(".png", img)
    assert ok
    buf.tofile(str(path))


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    h.update(path.read_bytes())
    return h.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser(description="摇尾巴帧序列后处理")
    ap.add_argument("--src", required=True, help="抠像原幅 frames 目录（960×960 透明）")
    ap.add_argument("--anchor", required=True, help="呼吸锚点帧 PNG（588×588 RGBA）")
    ap.add_argument("--start", type=int, required=True, help="起始帧号（抠像输出帧号，0 起）")
    ap.add_argument("--end", type=int, required=True, help="结束帧号（含）")
    ap.add_argument("--out", required=True, help="输出目录")
    args = ap.parse_args()

    src = Path(args.src)
    anchor_path = Path(args.anchor)
    out = Path(args.out)
    frames_out = out / "frames"
    frames_out.mkdir(parents=True, exist_ok=True)

    anchor = imread_rgba(anchor_path)
    assert anchor is not None, f"锚点帧读取失败: {anchor_path}"
    assert anchor.shape[:2] == (OUT_SIZE, OUT_SIZE), \
        f"锚点帧尺寸 {anchor.shape[:2]} != {(OUT_SIZE, OUT_SIZE)}"

    names = sorted((src).glob("f*.png"))
    assert names, f"源目录没有帧: {src}"

    # 选段（帧号 start..end 含）
    picks = [n for n in names if args.start <= int(n.stem[1:]) <= args.end]
    assert picks, f"选段 {args.start}-{args.end} 为空"

    print(f"[选段] 抠像帧 {args.start}-{args.end}（源视频帧 {args.start + 3}-{args.end + 3}）"
          f" 共 {len(picks)} 帧")
    print(f"[裁剪] x={CROP_X} y={CROP_Y} size={CROP_S} → resize {OUT_SIZE}（呼吸同框）")

    seq = []
    for p in picks:
        img = imread_rgba(p)
        assert img is not None, f"帧读取失败: {p}"
        crop = img[CROP_Y:CROP_Y + CROP_S, CROP_X:CROP_X + CROP_S]
        small = cv2.resize(crop, (OUT_SIZE, OUT_SIZE), interpolation=cv2.INTER_AREA)
        seq.append(small)

    # 首尾锚点替换（动作单元结构：f0000=锚点, 动作帧..., 末帧=锚点）
    total = len(seq) + 2
    seq = [anchor.copy()] + seq + [anchor.copy()]

    # 落盘
    files = []
    for i, img in enumerate(seq):
        fp = frames_out / f"f{i:04d}.png"
        imwrite_rgba(fp, img)
        files.append({"role": "frame", "relativePath": f"frames/f{i:04d}.png",
                      "sha256": sha256_of(fp)})
    print(f"[输出] {total} 帧 × {FRAME_MS}ms = {total * FRAME_MS}ms -> {frames_out}")

    # ---- 自检 ----
    print("\n" + "=" * 64)
    # 1. 首尾帧与锚点逐字节一致
    first = imread_rgba(frames_out / "f0000.png")
    last = imread_rgba(frames_out / f"f{total - 1:04d}.png")
    d_first = int(np.abs(first.astype(np.int16) - anchor.astype(np.int16)).max())
    d_last = int(np.abs(last.astype(np.int16) - anchor.astype(np.int16)).max())
    print(f"[自检1] 首帧与锚点最大像素差 {d_first}（应=0）  "
          f"{'PASS' if d_first == 0 else 'FAIL'}")
    print(f"[自检1] 末帧与锚点最大像素差 {d_last}（应=0）  "
          f"{'PASS' if d_last == 0 else 'FAIL'}")

    # 2. 轮廓面积波动（帧间前景面积变化）
    areas = []
    for i in range(total):
        img = seq[i]
        a = img[:, :, 3] >= 128
        areas.append(int(a.sum()))
    areas = np.array(areas, dtype=np.float64)
    base_area = areas[0]
    max_rel = float(np.abs(areas - base_area).max() / base_area)
    print(f"[自检2] 前景面积 base={int(base_area)}  最大相对波动 {max_rel:.4%}  "
          f"{'PASS' if max_rel <= 0.06 else 'FAIL'}（阈值 6%，尾巴摆动会带来轮廓变化）")

    # 3. 触边（动作帧不能贴 588 画布边）
    TOUCH = 2
    touched = []
    for i in range(total):
        a = seq[i][:, :, 3] >= 128
        if (a[:, :TOUCH].any() or a[:, -TOUCH:].any()
                or a[:TOUCH, :].any() or a[-TOUCH:, :].any()):
            touched.append(i)
    print(f"[自检3] 触边帧 {len(touched)}/{total}  "
          f"{'PASS' if not touched else 'FAIL: ' + str(touched[:10])}")

    # 4. 帧间不闪烁（相邻帧面积跳变）
    area_jumps = np.abs(np.diff(areas)) / base_area
    max_jump = float(area_jumps.max())
    print(f"[自检4] 相邻帧面积最大跳变 {max_jump:.5f}  "
          f"{'PASS' if max_jump <= 0.02 else 'FAIL'}（阈值 2%）")

    # manifest（v7 风格片段，供合并脚本消费）
    manifest = {
        "renderer": "frame-sequence-v1",
        "actionId": "tail",
        "loop": False,
        "frameDurationMs": FRAME_MS,
        "frameCount": total,
        "durationMs": total * FRAME_MS,
        "sourceVideo": "03-视频/02-摇尾巴.mp4",
        "sourceFrames": [args.start + 3, args.end + 3],
        "crop": {"x": CROP_X, "y": CROP_Y, "size": CROP_S, "outputSize": OUT_SIZE},
        "selfCheck": {
            "firstLastAnchorDiff": [d_first, d_last],
            "maxAreaFluctuationRel": round(max_rel, 6),
            "touchedFrames": touched,
            "maxAreaJump": round(max_jump, 6),
        },
        "files": files,
    }
    (out / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n[输出] {out / 'manifest.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

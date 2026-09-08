# -*- coding: utf-8 -*-
"""
窗口裁剪轮廓体检：验证"静态首帧轮廓"到底裁掉了多少内容。

背景：Windows SetWindowRgn 按 hit surface 的 alpha 裁剪窗口绘制区。
旧实现 hit surface 永远画 baseImage(f0000)，动作期间身体形变超出首帧轮廓的
像素会被窗口裁掉（用户反馈"内容显示不全"）。

新实现：hit surface 画"当前动作所有帧的并集轮廓"。
本脚本量化：
  1. 每个动作的并集 bbox 与首帧 bbox 的差异（窗口需要多出多少像素）
  2. 旧轮廓会裁掉的像素量（并集 - 首帧），分动作统计
  3. 输出证据图：首帧轮廓 / 并集轮廓 / 被裁掉的区域（红）

用法：
  D:\\DevTools\\Python312\\python.exe scripts/poc_窗口裁剪轮廓体检.py
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
PETS = ROOT / "apps" / "desktop" / "public" / "builtin-pets"
OUT = ROOT / "output" / "窗口裁剪轮廓体检"
OUT.mkdir(parents=True, exist_ok=True)


def imread(path: Path) -> np.ndarray:
    data = np.fromfile(str(path), dtype=np.uint8)
    return cv2.imdecode(data, cv2.IMREAD_UNCHANGED)


def alpha_mask(img: np.ndarray, threshold: int = 32) -> np.ndarray:
    """取 alpha 通道并按阈值二值化（与前端 alphaToRegionSpans 的 alphaThreshold=32 对齐）。"""
    if img.ndim == 2:
        return np.full(img.shape, 255, dtype=np.uint8)
    a = img[:, :, 3]
    return np.where(a >= threshold, 255, 0).astype(np.uint8)


def union_mask(paths: list[Path], threshold: int = 32) -> np.ndarray:
    """并集轮廓：任意帧不透明即不透明（对应 canvas "lighter" 的 alpha 合成）。"""
    acc: np.ndarray | None = None
    for path in paths:
        m = alpha_mask(imread(path), threshold)
        acc = m if acc is None else cv2.bitwise_or(acc, m)
    if acc is None:
        raise ValueError("no frames")
    return acc


def bbox_of(mask: np.ndarray) -> tuple[int, int, int, int]:
    ys, xs = np.nonzero(mask)
    if len(xs) == 0:
        return (0, 0, 0, 0)
    return (int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1)


def frames_root(pet: str) -> Path:
    """老资产用 `actions/`，新资产用 `frames/`。"""
    pet_dir = PETS / pet
    for name in ("frames", "actions"):
        if (pet_dir / name).is_dir():
            return pet_dir / name
    raise FileNotFoundError(f"{pet} 没有 frames/ 或 actions/ 目录")


def action_frames(pet: str, action: str) -> list[Path]:
    d = frames_root(pet) / action
    return sorted(d.glob("f*.png"))


def resolve_base_image(pet: str) -> Path:
    """取旧实现真正用的那张图：manifest.baseImage，缺失时退回默认动作首帧。"""
    import json

    manifest = json.loads((PETS / pet / "manifest.json").read_text(encoding="utf-8"))
    base = manifest.get("baseImage")
    if isinstance(base, str) and base:
        candidate = PETS / pet / base
        if candidate.exists():
            return candidate
    return action_frames(pet, manifest["defaultAction"])[0]


def analyze(pet: str) -> None:
    print(f"\n=== {pet} ===")
    actions = sorted(p.name for p in frames_root(pet).glob("*") if p.is_dir())
    base_image = resolve_base_image(pet)

    first = alpha_mask(imread(base_image))
    first_bbox = bbox_of(first)
    print(f"首帧基准 = {base_image.relative_to(PETS)}")
    print(f"首帧轮廓 bbox = {first_bbox}  (x0,y0,x1,y1)  面积={first.sum() // 255}")

    canvas_h, canvas_w = first.shape
    evidence: list[np.ndarray] = []

    for action in actions:
        frames = action_frames(pet, action)
        uni = union_mask(frames)
        uni_bbox = bbox_of(uni)
        clipped = cv2.bitwise_and(uni, cv2.bitwise_not(first))  # 旧轮廓会裁掉的部分
        clipped_px = int(clipped.sum() // 255)
        ratio = clipped_px / max(1, int(uni.sum() // 255))

        dx0 = first_bbox[0] - uni_bbox[0]
        dy0 = first_bbox[1] - uni_bbox[1]
        dx1 = uni_bbox[2] - first_bbox[2]
        dy1 = uni_bbox[3] - first_bbox[3]

        print(f"\n[{action}] {len(frames)} 帧")
        print(f"  并集 bbox        = {uni_bbox}  面积={int(uni.sum() // 255)}")
        print(f"  相对首帧外扩(左,上,右,下) = ({dx0}, {dy0}, {dx1}, {dy1}) px")
        print(f"  旧轮廓裁掉的像素 = {clipped_px} ({ratio * 100:.2f}% of 并集)")

        # 证据图：左=首帧轮廓，中=并集轮廓，右=首帧之外会被裁掉的区域(红)
        panel = np.zeros((canvas_h, canvas_w, 3), dtype=np.uint8)
        panel[:, :, 0] = first  # 蓝 = 旧轮廓
        panel[:, :, 1] = uni    # 绿 = 并集轮廓
        red = np.zeros_like(panel)
        red[:, :, 2] = clipped
        panel = cv2.add(panel, red)
        panel = cv2.resize(panel, (canvas_w // 2, canvas_h // 2), interpolation=cv2.INTER_AREA)
        label = f"{pet} / {action}  clipped={clipped_px}px ({ratio * 100:.2f}%)"
        cv2.putText(panel, label, (8, 20), cv2.FONT_HERSHEY_SIMPLEX, 0.45, (255, 255, 255), 1, cv2.LINE_AA)
        evidence.append(panel)

    grid = np.vstack(evidence)
    out = OUT / f"{pet}-裁剪轮廓体检.png"
    ok, buf = cv2.imencode(".png", grid)
    if not ok:
        raise RuntimeError("encode failed")
    buf.tofile(str(out))
    print(f"\n证据图 -> {out}")


def main() -> int:
    parser = argparse.ArgumentParser(description="窗口裁剪轮廓体检")
    parser.add_argument("--pet", action="append", default=[], help="宠物目录名，可重复；默认全部")
    args = parser.parse_args()

    pets = args.pet or sorted(p.name for p in PETS.glob("*"))
    for pet in pets:
        if not ((PETS / pet / "frames").exists() or (PETS / pet / "actions").exists()):
            print(f"[跳过] {pet} 没有 frames/ 或 actions/ 目录")
            continue
        analyze(pet)
    return 0


if __name__ == "__main__":
    sys.exit(main())

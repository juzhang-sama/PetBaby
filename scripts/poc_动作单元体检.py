#!/usr/bin/env python3
"""动作单元体检：判断一支候选绿幕视频能不能当"动作单元"用。

为什么需要这个脚本
------------------
动作单元的硬约束是：**首帧和尾帧都必须是同一张"静止锚点帧"**，运行时才能把它
插进呼吸循环里而不跳。但 Seedance 生成的结果不可控——它可能：

  - 第一帧就开始动（没有静止起手）
  - 结尾没收敛回锚点姿态（尾帧和首帧对不上）
  - 动的部位不是我们想要的（让他摇尾巴，结果整只猫在晃）
  - 甩出画面被裁（尾巴触边）

这些用眼睛扫一遍看不出来，但数字能看出来。所以先体检、再抠像，避免把一支
不合格的视频走完整个流程才发现不能用。

九项判据
--------
1. 锚点匹配度   —— 视频里必须存在某一帧，与锚点帧 alpha 的 IoU 足够高（>= 0.97）
2. 首帧即锚点   —— 最佳匹配帧应该就在开头（<= 3 帧），否则运行时接不上
3. 尾帧收敛     —— 最后 K 帧的帧间 alpha 差分要接近 0
4. 首尾一致     —— 尾帧与最佳匹配帧的 IoU 要够高（>= 0.97）
5. 运动部位     —— 若指定 --expect-side，运动重心的 x 必须落在期望一侧
6. 无触边裁切   —— 任何一帧的前景都不能贴到画布边界
7. 动作已发生   —— 峰值帧差分必须够大，否则说明模型压根没响应
8. 区域集中度   —— 若指定 --expect-region，差分像素必须集中在期望矩形内（防加戏）
9. 垂直方向     —— 若指定 --expect-vertical，头顶最高点必须朝期望方向移动

为什么要有 7/8/9
----------------
前六项全是"一致性"判据，它们回答的问题是「这支视频能不能接回 idle」。
但它们**回答不了「动作做没做对」**——一支模型完全没响应、全程纹丝不动的视频，
首尾一致、尾段收敛、不触边全都满足，六项全 PASS，然后我们高高兴兴拿去抠像，
抠完才发现这是一支 5 秒静止画面。

动耳朵是全系列里幅度最小的单元，这个坑几乎是必然要踩的。所以补了 7/8/9：

  - 7 防「模型不响应」（全程静止）
  - 8 防「模型加戏」（让摇尾巴结果整只猫都在晃）
  - 9 防「模型理解反了」（让点头变成抬头）

阈值量级参考（实测）
--------------------
峰值帧差分 peakDiffRel（动作最强那一帧的 alpha 变化量 ÷ 前景面积）：

  - 摇尾巴（废弃候选 video_1.mp4 实测）  ：0.0276
  - 点头（估算，头部面积大但只转 5-8 度）：0.005 ~ 0.015
  - 动耳朵（估算，只有耳朵那小块在动）  ：0.0006 ~ 0.003

注意不要用平均差分判动作强度：合格单元有 2/3 时长是静止段，
平均会被稀释到接近 0，只有**峰值**才是动作强度的正确度量。

用法
----
    python scripts/poc_动作单元体检.py ^
        --video <候选视频> ^
        --anchor <锚点帧 PNG（抠像后的 RGBA）> ^
        --key-params <呼吸循环那支的 抠像参数.json> ^
        --out <输出目录>

判据全过就打印「可作为动作单元」，否则打印具体哪一项没过、以及建议。
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import cv2
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from poc_抠像 import (  # noqa: E402
    KEY_HIGH,
    KEY_LOW,
    UNMIX_MIN_ALPHA,
    chroma_alpha,
    clean_mask,
    despill,
    estimate_background,
    extract_frames,
    probe_fps,
    probe_size,
    spatial_smooth,
)

# ---------------------------------------------------------------- 判据阈值

ANCHOR_IOU_MIN = 0.97      # 与锚点帧的轮廓重合度下限
ANCHOR_INDEX_MAX = 3       # 最佳匹配帧允许出现的最靠后位置（帧）
TAIL_IOU_MIN = 0.97        # 尾帧与最佳匹配帧的重合度下限
TAIL_MOTION_MAX = 0.004    # 最后 K 帧的平均帧间 alpha 差分上限（占前景比例）
TOUCH_MARGIN = 2           # 距离画布边界多少像素算"触边"
TAIL_WINDOW = 12           # 判定收敛时看最后多少帧

MIN_PEAK_MOTION = 0.0005   # 峰值帧差分下限（防"模型不响应"）。
                           # 摇尾实测 0.0276，动耳朵估算 0.0006~0.003，
                           # 取 0.0005 才能覆盖全系列里幅度最小的单元
MIN_REGION_RATIO = 0.80    # 差分像素落在期望矩形内的比例下限（防"模型加戏"）
MIN_VERTICAL_SHIFT = 4     # 头顶最高点的最小位移量（像素，防"没真动"）
HEADROOM_TOLERANCE = 2     # 允许头顶比锚点帧高出的像素数（防"仰头冲过头"）


# ---------------------------------------------------------------- 工具

def imread_rgba(path: Path) -> np.ndarray:
    """读 RGBA，兼容中文路径（cv2.imread 读不了）。"""
    buf = np.fromfile(str(path), np.uint8)
    img = cv2.imdecode(buf, cv2.IMREAD_UNCHANGED)
    if img is None:
        raise SystemExit(f"读不了图片: {path}")
    if img.ndim == 2:
        img = cv2.cvtColor(img, cv2.COLOR_GRAY2RGBA)
    elif img.shape[2] == 3:
        img = cv2.cvtColor(img, cv2.COLOR_BGRA2RGBA)
    else:
        img = cv2.cvtColor(img, cv2.COLOR_BGRA2RGBA)
    return img


def alpha_mask(rgba: np.ndarray, thresh: float = 0.5) -> np.ndarray:
    if rgba.shape[2] < 4:
        raise SystemExit("锚点帧必须是带 alpha 的 PNG")
    return (rgba[:, :, 3].astype(np.float32) / 255.0) >= thresh


def iou(a: np.ndarray, b: np.ndarray) -> float:
    inter = float(np.logical_and(a, b).sum())
    union = float(np.logical_or(a, b).sum())
    return inter / union if union else 0.0


def load_key_params(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    crop = data.get("crop") or {}
    return {
        "sourceSize": tuple(data.get("sourceSize", (0, 0))),
        "cropX": int(crop.get("x", 0)),
        "cropY": int(crop.get("y", 0)),
        "cropSize": int(crop.get("size", 0)),
        "spatialSigma": float(data.get("spatialSigma", 0.6)),
    }


# ---------------------------------------------------------------- 主流程

def build_alpha_stack(rgbs: list[np.ndarray], bg_ref: np.ndarray,
                      sigma: float) -> list[np.ndarray]:
    """色键 -> 去溢色无关，这里只关心 alpha。"""
    stack = []
    for rgb in rgbs:
        bg_alpha = chroma_alpha(rgb)
        alpha = 1.0 - bg_alpha
        alpha = clean_mask(alpha)
        if sigma > 0:
            alpha = spatial_smooth(alpha, sigma)
        stack.append(np.clip(alpha, 0.0, 1.0))
    return stack


def crop_all(rgbs: list[np.ndarray], alphas: list[np.ndarray],
             x: int, y: int, size: int) -> tuple[list[np.ndarray], list[np.ndarray]]:
    cr, ca = [], []
    for rgb, a in zip(rgbs, alphas):
        cr.append(rgb[y:y + size, x:x + size])
        ca.append(a[y:y + size, x:x + size])
    return cr, ca


def motion_map(prev: np.ndarray, cur: np.ndarray) -> np.ndarray:
    return np.abs(cur - prev)


def accumulate_motion(stack: list[np.ndarray]) -> np.ndarray:
    """把所有帧的帧间差分累加成一张"运动热力图"。

    必须用 alpha 而不是 RGB：绿幕区域的 RGB 会被 H.264 噪点污染，
    按 RGB 差分会把噪点当成运动（早前一次基于 RGB 的分析就因此误判过）。
    """
    acc = np.zeros_like(stack[0], dtype=np.float32)
    for i in range(len(stack) - 1):
        acc += motion_map(stack[i], stack[i + 1])
    return acc


def region_ratio(acc: np.ndarray, region: tuple[float, float, float, float]) -> float:
    """差分像素落在期望矩形内的比例（防"模型加戏"）。

    region 为归一化 (x, y, w, h)，相对裁剪后的画布。
    """
    h, w = acc.shape
    x0 = max(0, int(round(region[0] * w)))
    y0 = max(0, int(round(region[1] * h)))
    x1 = min(w, int(round((region[0] + region[2]) * w)))
    y1 = min(h, int(round((region[1] + region[3]) * h)))
    total = float(acc.sum())
    if total <= 0 or x1 <= x0 or y1 <= y0:
        return 0.0
    return float(acc[y0:y1, x0:x1].sum() / total)


def head_top_series(masks: list[np.ndarray]) -> list[int]:
    """逐帧求前景最高点的 y 坐标（头顶高度）。

    y 增大 = 头顶下移（低头）；y 减小 = 头顶上移（抬头）。
    点头单元靠这个判方向，防模型理解反了。
    """
    tops: list[int] = []
    for m in masks:
        rows = np.flatnonzero(m.any(axis=1))
        tops.append(int(rows[0]) if rows.size else int(m.shape[0]))
    return tops


def analyze_motion(stack: list[np.ndarray]) -> tuple[dict, np.ndarray]:
    """基于 alpha 的帧间差分做运动分析（绿幕 RGB 差分会被噪点污染，必须用 alpha）。

    返回 (指标字典, 运动热力图)。热力图给区域集中度判据复用，避免重复累加。
    """
    n = len(stack)
    diffs = [float(motion_map(stack[i], stack[i + 1]).sum()) for i in range(n - 1)]
    fg_area = float(stack[0].sum())
    rel = [d / max(fg_area, 1.0) for d in diffs]

    # 空间分布：把所有帧的差分图累加，再按列/行投影
    acc = accumulate_motion(stack)
    h, w = acc.shape
    col_prof = acc.sum(axis=0)
    row_prof = acc.sum(axis=1)
    total = float(acc.sum()) or 1.0

    third = w // 3
    thirds = {
        "左": float(col_prof[:third].sum() / total),
        "中": float(col_prof[third:2 * third].sum() / total),
        "右": float(col_prof[2 * third:].sum() / total),
    }
    xs = np.arange(w, dtype=np.float32)
    ys = np.arange(h, dtype=np.float32)
    cx = float((col_prof * xs).sum() / total) / w
    cy = float((row_prof * ys).sum() / total) / h

    # 每帧运动强度（定位动作发生在哪一段）
    per_frame = [0.0] + rel
    peak = int(np.argmax(per_frame))

    return {
        "foregroundArea": round(fg_area, 1),
        "meanFrameDiffRel": round(float(np.mean(rel)), 6),
        "maxFrameDiffRel": round(float(np.max(rel)) if rel else 0.0, 6),
        "thirds": {k: round(v, 4) for k, v in thirds.items()},
        "motionCentroid": {"x": round(cx, 4), "y": round(cy, 4)},
        "peakFrame": peak,
        "peakDiffRel": round(per_frame[peak], 6),
    }, acc


def check_touch(stack: list[np.ndarray]) -> dict:
    """检查是否有帧的前景贴到画布边界（说明动作甩出画被裁了）。"""
    h, w = stack[0].shape
    touched = []
    for i, a in enumerate(stack):
        m = a >= 0.5
        if (m[:, :TOUCH_MARGIN].any() or m[:, -TOUCH_MARGIN:].any()
                or m[:TOUCH_MARGIN, :].any() or m[-TOUCH_MARGIN:, :].any()):
            touched.append(i)
    return {"touchedFrames": touched, "touchCount": len(touched),
            "totalFrames": len(stack)}


def main() -> int:
    ap = argparse.ArgumentParser(description="动作单元体检")
    ap.add_argument("--video", required=True, help="候选绿幕视频")
    ap.add_argument("--anchor", required=True, help="锚点帧 PNG（抠像后 588×588 RGBA）")
    ap.add_argument("--key-params", required=True, help="参照支的 抠像参数.json（复用裁剪框）")
    ap.add_argument("--out", required=True, help="输出目录")
    ap.add_argument("--expect-side", choices=["left", "right", "none"], default="none",
                    help="期望的运动重心在哪一侧（摇尾巴通常是 right）")
    ap.add_argument("--min-peak-motion", type=float, default=MIN_PEAK_MOTION,
                    help=f"峰值帧差分下限，防模型不响应（默认 {MIN_PEAK_MOTION}）")
    ap.add_argument("--expect-region", default="",
                    help="期望的运动区域，归一化 x,y,w,h（防加戏）。例 0.55,0.05,0.35,0.35")
    ap.add_argument("--min-region-ratio", type=float, default=MIN_REGION_RATIO,
                    help=f"差分落在期望区域内的比例下限（默认 {MIN_REGION_RATIO}）")
    ap.add_argument("--expect-vertical", choices=["down", "up", "none"], default="none",
                    help="期望头顶最高点朝哪个方向移动（点头用 down，防模型做成抬头）")
    ap.add_argument("--fps", type=float, default=0.0, help="抽帧帧率，0=跟随源视频")
    args = ap.parse_args()

    expect_region: tuple[float, float, float, float] | None = None
    if args.expect_region:
        parts = [p.strip() for p in args.expect_region.split(",")]
        if len(parts) != 4:
            raise SystemExit("--expect-region 需要 4 个逗号分隔的数：x,y,w,h（归一化）")
        try:
            expect_region = tuple(float(p) for p in parts)  # type: ignore[assignment]
        except ValueError:
            raise SystemExit(f"--expect-region 解析失败: {args.expect_region}")
        if not all(0.0 <= v <= 1.0 for v in expect_region):
            raise SystemExit(f"--expect-region 四个值都必须落在 0~1: {args.expect_region}")

    video = Path(args.video)
    anchor_path = Path(args.anchor)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    kp = load_key_params(Path(args.key_params))
    anchor_rgba = imread_rgba(anchor_path)
    anchor_a = alpha_mask(anchor_rgba)
    anchor_size = anchor_rgba.shape[0]
    print(f"锚点帧: {anchor_path.name}  {anchor_rgba.shape[1]}x{anchor_rgba.shape[0]}  "
          f"前景面积={int(anchor_a.sum())}")

    src_size = probe_size(video)
    src_fps = probe_fps(video)
    ref_size = kp["sourceSize"]
    print(f"视频: {video.name}  {src_size[0]}x{src_size[1]}  fps={src_fps:.2f}")
    # 跨画幅支持：Seedance 1:1 输出从 640 升到 960 后，所有新视频都会与旧参照支画幅不同。
    # 首帧构图一致（同一张 1024 首帧缩放而来），裁剪框按比例归一化换算即可复用。
    crop_scale = 1.0
    if ref_size and tuple(src_size) != tuple(ref_size):
        if src_size[0] / src_size[1] != ref_size[0] / ref_size[1]:
            print(f"[中止] 画幅长宽比不一致：候选 {src_size} vs 参照 {ref_size}。"
                  f"非等比缩放无法换算裁剪框，必须重新生成。")
            return 1
        crop_scale = src_size[0] / ref_size[0]
        print(f"[跨画幅] 候选 {src_size[0]}x{src_size[1]} vs 参照 {ref_size[0]}x{ref_size[1]}，"
              f"裁剪框按 {crop_scale:.4f}x 归一化换算（构图同源）")

    work = Path(tempfile.mkdtemp(prefix="motion-check-"))
    try:
        fps = args.fps if args.fps > 0 else src_fps
        frame_paths = extract_frames(video, work, fps)
        rgbs = []
        for p in frame_paths:
            buf = np.fromfile(str(p), np.uint8)
            img = cv2.imdecode(buf, cv2.IMREAD_COLOR)
            rgbs.append(cv2.cvtColor(img, cv2.COLOR_BGR2RGB))
        print(f"抽帧: {len(rgbs)} 帧 @ {fps:.2f}fps")

        bg_ref = estimate_background(rgbs)
        print(f"实测绿幕色: RGB=({bg_ref[0]:.0f},{bg_ref[1]:.0f},{bg_ref[2]:.0f})")

        alphas = build_alpha_stack(rgbs, bg_ref, kp["spatialSigma"])

        # 复用参照支的裁剪框（跨画幅时按比例换算）
        if kp["cropSize"]:
            x = int(round(kp["cropX"] * crop_scale))
            y = int(round(kp["cropY"] * crop_scale))
            size = int(round(kp["cropSize"] * crop_scale))
        else:
            size = min(rgbs[0].shape[:2])
            x = (rgbs[0].shape[1] - size) // 2
            y = (rgbs[0].shape[0] - size) // 2
        # 裁剪框不能越界（换算舍入可能 +1px）
        size = min(size, rgbs[0].shape[1] - x, rgbs[0].shape[0] - y)
        print(f"裁剪框（复用参照支）: x={x} y={y} size={size}")
        if size < anchor_size:
            print(f"[中止] 裁剪后尺寸 {size} 小于锚点帧 {anchor_size}，放大无可信度。")
            return 1

        rgbs, alphas = crop_all(rgbs, alphas, x, y, size)

        # 跨画幅：裁剪后 882 之类的中间尺寸 resize 回锚点 588，保证 IoU 可比。
        # INTER_AREA 缩小是可信的（信息有损但保形）；只在尺寸不等时做。
        if size != anchor_size:
            print(f"[resize] 裁剪后 {size}x{size} → 锚点 {anchor_size}x{anchor_size}（INTER_AREA）")
            resized_rgbs, resized_alphas = [], []
            for rgb, a in zip(rgbs, alphas):
                resized_rgbs.append(cv2.resize(rgb, (anchor_size, anchor_size),
                                               interpolation=cv2.INTER_AREA))
                resized_alphas.append(cv2.resize(a, (anchor_size, anchor_size),
                                                interpolation=cv2.INTER_AREA))
            rgbs, alphas = resized_rgbs, resized_alphas

        # ---- 判据 1 & 2：锚点匹配
        masks = [a >= 0.5 for a in alphas]
        ious = [iou(m, anchor_a) for m in masks]
        best_i = int(np.argmax(ious))
        best_iou = ious[best_i]
        print("\n" + "=" * 64)
        print(f"[1] 锚点匹配度    最佳帧 #{best_i}  IoU={best_iou:.4f}  "
              f"{'PASS' if best_iou >= ANCHOR_IOU_MIN else 'FAIL'} (阈值 {ANCHOR_IOU_MIN})")
        print(f"[2] 首帧即锚点    最佳帧位置={best_i}  "
              f"{'PASS' if best_i <= ANCHOR_INDEX_MAX else 'FAIL'} (阈值 <= {ANCHOR_INDEX_MAX})")

        ref_mask = masks[best_i]

        # ---- 判据 4：尾帧收敛到锚点
        tail_iou = iou(masks[-1], ref_mask)
        print(f"[4] 尾帧回到锚点  末帧 IoU={tail_iou:.4f}  "
              f"{'PASS' if tail_iou >= TAIL_IOU_MIN else 'FAIL'} (阈值 {TAIL_IOU_MIN})")

        # ---- 判据 3：最后 K 帧收敛
        k = min(TAIL_WINDOW, len(alphas) - 1)
        tail_diffs = [float(motion_map(alphas[i], alphas[i + 1]).sum())
                      / max(float(ref_mask.sum()), 1.0)
                      for i in range(len(alphas) - k, len(alphas) - 1)]
        tail_mean = float(np.mean(tail_diffs)) if tail_diffs else 0.0
        print(f"[3] 尾段收敛      最后 {k} 帧平均差分={tail_mean:.6f}  "
              f"{'PASS' if tail_mean <= TAIL_MOTION_MAX else 'FAIL'} (阈值 {TAIL_MOTION_MAX})")

        # ---- 判据 5：运动部位
        motion, motion_acc = analyze_motion(alphas)
        side_ok = True
        if args.expect_side != "none":
            cx = motion["motionCentroid"]["x"]
            side_ok = cx < 0.5 if args.expect_side == "left" else cx > 0.5
        print(f"[5] 运动部位      重心 x={motion['motionCentroid']['x']:.3f} "
              f"y={motion['motionCentroid']['y']:.3f}  "
              f"三分区 左{motion['thirds']['左']:.2f} 中{motion['thirds']['中']:.2f} "
              f"右{motion['thirds']['右']:.2f}  "
              f"{'PASS' if side_ok else 'FAIL'}"
              + ("" if args.expect_side == "none" else f" (期望 {args.expect_side})"))

        # ---- 判据 6：触边
        touch = check_touch(masks)
        print(f"[6] 无触边裁切    {touch['touchCount']}/{touch['totalFrames']} 帧触边  "
              f"{'PASS' if touch['touchCount'] == 0 else 'FAIL'}"
              + ("" if touch["touchCount"] == 0 else f"  前 10 帧: {touch['touchedFrames'][:10]}"))

        # ---- 判据 7：动作到底发生没有（防"模型不响应"）
        peak_rel = motion["peakDiffRel"]
        motion_ok = peak_rel >= args.min_peak_motion
        print(f"[7] 动作已发生    峰值帧 #{motion['peakFrame']} 差分={peak_rel:.6f}  "
              f"{'PASS' if motion_ok else 'FAIL'} (阈值 {args.min_peak_motion})")
        if not motion_ok:
            print(f"    峰值都没到阈值 = 模型基本没动。整段平均 {motion['meanFrameDiffRel']:.6f}，"
                  f"别急着抠像，先确认提示词有没有被响应。")

        # ---- 判据 8：运动区域集中度（防"模型加戏"）
        region_ratio_v: float | None = None
        region_ok = True
        if expect_region is not None:
            region_ratio_v = region_ratio(motion_acc, expect_region)
            region_ok = region_ratio_v >= args.min_region_ratio
            print(f"[8] 区域集中度    落在期望矩形内 {region_ratio_v:.2%}  "
                  f"{'PASS' if region_ok else 'FAIL'} (阈值 {args.min_region_ratio:.0%})")
            if not region_ok:
                print(f"    另有 {1 - region_ratio_v:.2%} 的差分落在矩形外 —— 模型加戏了，"
                      f"动到了不该动的部位。")
        else:
            print("[8] 区域集中度    跳过（未指定 --expect-region）")

        # ---- 判据 9：垂直方向（防"点头做成抬头"）
        tops = head_top_series(masks)
        anchor_top = head_top_series([anchor_a])[0]
        top_min, top_max = min(tops), max(tops)
        vertical_ok = True
        if args.expect_vertical == "down":
            shift = top_max - anchor_top       # 低头：头顶下移，y 增大
            overshoot = anchor_top - top_min   # 仰头冲过头：y 比锚点还小
            vertical_ok = shift >= MIN_VERTICAL_SHIFT and overshoot <= HEADROOM_TOLERANCE
            print(f"[9] 垂直方向      锚点头顶 y={anchor_top}，全程 y={top_min}~{top_max}  "
                  f"{'PASS' if vertical_ok else 'FAIL'}")
            print(f"    期望低头：下移 {shift}px (需 >={MIN_VERTICAL_SHIFT})，"
                  f"反向仰头 {overshoot}px (需 <={HEADROOM_TOLERANCE})")
        elif args.expect_vertical == "up":
            shift = anchor_top - top_min
            overshoot = top_max - anchor_top
            vertical_ok = shift >= MIN_VERTICAL_SHIFT and overshoot <= HEADROOM_TOLERANCE
            print(f"[9] 垂直方向      锚点头顶 y={anchor_top}，全程 y={top_min}~{top_max}  "
                  f"{'PASS' if vertical_ok else 'FAIL'}")
            print(f"    期望抬头：上移 {shift}px (需 >={MIN_VERTICAL_SHIFT})，"
                  f"反向低头 {overshoot}px (需 <={HEADROOM_TOLERANCE})")
        else:
            print(f"[9] 垂直方向      跳过（未指定 --expect-vertical）  "
                  f"锚点头顶 y={anchor_top}，全程 y={top_min}~{top_max}")

        # ---- 汇总
        checks = {
            "锚点匹配度": best_iou >= ANCHOR_IOU_MIN,
            "首帧即锚点": best_i <= ANCHOR_INDEX_MAX,
            "尾段收敛": tail_mean <= TAIL_MOTION_MAX,
            "尾帧回到锚点": tail_iou >= TAIL_IOU_MIN,
            "运动部位": side_ok,
            "无触边裁切": touch["touchCount"] == 0,
            "动作已发生": motion_ok,
            "区域集中度": region_ok,
            "垂直方向": vertical_ok,
        }
        passed = all(checks.values())
        print("=" * 64)
        print("结论: " + ("✅ 可作为动作单元" if passed else "❌ 不可作为动作单元"))
        if not passed:
            print("未通过项:")
            for name, ok in checks.items():
                if not ok:
                    print(f"  - {name}")

        # ---- 输出 JSON + 首尾对照图
        report = {
            "video": str(video),
            "videoSha256": "",
            "anchor": str(anchor_path),
            "sourceSize": list(src_size),
            "sourceFps": src_fps,
            "frameCount": len(alphas),
            "measuredBackgroundRGB": [round(float(v), 2) for v in bg_ref],
            "crop": {"x": x, "y": y, "size": size},
            "keyLow": KEY_LOW,
            "keyHigh": KEY_HIGH,
            "unmixMinAlpha": UNMIX_MIN_ALPHA,
            "checks": {k: bool(v) for k, v in checks.items()},
            "overallPassed": bool(passed),
            "metrics": {
                "anchorBestFrame": best_i,
                "anchorBestIoU": round(best_iou, 4),
                "tailIoU": round(tail_iou, 4),
                "tailMeanDiffRel": round(tail_mean, 6),
                "motion": motion,
                "peakMotion": {
                    "peakDiffRel": round(peak_rel, 6),
                    "minPeakMotion": args.min_peak_motion,
                },
                "regionConcentration": {
                    "expectRegion": list(expect_region) if expect_region else None,
                    "ratio": (round(region_ratio_v, 4)
                              if region_ratio_v is not None else None),
                    "minRegionRatio": args.min_region_ratio,
                },
                "headTop": {
                    "anchor": anchor_top,
                    "seriesMin": top_min,
                    "seriesMax": top_max,
                    "expectVertical": args.expect_vertical,
                    "minVerticalShift": MIN_VERTICAL_SHIFT,
                    "headroomTolerance": HEADROOM_TOLERANCE,
                },
                "touch": touch,
                "iouSeries": [round(v, 4) for v in ious],
            },
        }
        (out / "体检报告.json").write_text(
            json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")

        # 对照图：锚点 / 最佳匹配帧 / 尾帧 并排（alpha 合成到灰底）
        def to_bgra(a: np.ndarray) -> np.ndarray:
            m = (a >= 0.5).astype(np.uint8) * 255
            return cv2.cvtColor(m, cv2.COLOR_GRAY2BGRA)

        canvas = 255 * np.ones((anchor_size, anchor_size * 3, 3), np.float32)
        for idx, m in enumerate([anchor_a, ref_mask, masks[-1]]):
            canvas[:, idx * anchor_size:(idx + 1) * anchor_size][m] = (90, 90, 90)
        cv2.imencode(".png", canvas)[1].tofile(str(out / "锚点-最佳帧-尾帧.png"))

        print(f"\n输出: {out / '体检报告.json'}")
        print(f"输出: {out / '锚点-最佳帧-尾帧.png'}")
        return 0 if passed else 2
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

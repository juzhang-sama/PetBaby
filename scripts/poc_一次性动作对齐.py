# -*- coding: utf-8 -*-
"""一次性动作帧序列对齐：把新动作的抠像帧对齐到默认循环的画布坐标系。

为什么需要这个脚本
------------------
一次性动作（打哈欠/伸懒腰）和默认循环（idle-combo）是两支独立生成的视频。
若各自用 `poc_抠像.py --autocrop` 自动取景，会得到两个不同的裁剪框：

    05-silver-tabby 实测：idle-combo f0 bbox (18,31,497,551)
                          yawn       f0 bbox (36,33,533,571)
    -> 猫右移 27px、下移 20px、放大约 4%

一次性动作是在默认循环边界触发的（idleSchedule.alignToDefaultLoop），
这个错位会在触发瞬间变成肉眼可见的「猫跳一下」。

正确做法：新动作**复用默认循环的取景框**（同一支 640 源画布 + 同一张首帧，
所以同一个裁剪框能保证像素级对齐），而不是各自算自己的。

前置条件
--------
两支视频必须同源：同一张绿幕首帧、同一画幅（都是 640x640）。
若新动作的运动范围超出默认循环取景框，脚本会报硬失败——此时需要
重新裁剪两个动作（或用 --crop 手动放宽，代价是两个动作都要重出）。

用法
----
    D:/DevTools/Python312/python.exe scripts/poc_一次性动作对齐.py \
        --src output/.../10-打哈欠/01-抠像/frames \
        --ref-params output/.../09-组合循环/00-抠像/抠像参数.json \
        --anchor apps/desktop/public/builtin-pets/04-warm-brown-tabby/frames/idle-combo/f0000.png \
        --out output/.../10-打哈欠/02-帧序列

输出
----
    <out>/frames/f0000.png ...   对齐后的透明 PNG（--size x --size）
    <out>/manifest.json          单动作 manifest（供打包脚本消费）
    <out>/对齐自检.json          对齐指标

自检项
------
- 越界：新动作运动范围是否超出取景框（硬失败）
- 触边：是否有帧的主体贴到画布边缘（软警告）
- 锚点对齐：首帧与默认循环锚点帧的 bbox 偏移 + IoU
- 颜色一致：首帧与锚点帧的主体 RGB 均值差
- 面积波动：主体像素面积相对首帧的最大波动
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
FRAME_MS = 42

# 自检阈值
ALPHA_FG = 128        # 判定「不透明主体」的 alpha
IOU_WARN = 0.90       # 首帧与锚点的轮廓 IoU 下限
BBOX_SHIFT_WARN = 8   # 首帧与锚点的 bbox 偏移告警阈值（像素）
COLOR_SHIFT_WARN = 12.0  # 主体 RGB 均值差告警阈值
AREA_WAVE_WARN = 0.08    # 面积波动告警阈值


def imread_rgba(p: Path) -> np.ndarray:
    buf = np.fromfile(str(p), np.uint8)
    img = cv2.imdecode(buf, cv2.IMREAD_UNCHANGED)
    assert img is not None, f"读取失败 {p}"
    if img.shape[2] == 3:
        img = cv2.cvtColor(img, cv2.COLOR_BGR2RGBA)
    return img


def imwrite_rgba(p: Path, img: np.ndarray) -> None:
    ok, buf = cv2.imencode(".png", img)
    assert ok
    buf.tofile(str(p))


def sha256_of(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def fg_mask(img: np.ndarray, thr: int = ALPHA_FG) -> np.ndarray:
    return img[:, :, 3] >= thr


def bbox_of(mask: np.ndarray) -> tuple[int, int, int, int] | None:
    ys, xs = np.nonzero(mask)
    if ys.size == 0:
        return None
    return (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max()))


def iou(a: np.ndarray, b: np.ndarray) -> float:
    inter = int(np.count_nonzero(a & b))
    union = int(np.count_nonzero(a | b))
    return inter / union if union else 0.0


def mean_rgb(img: np.ndarray, mask: np.ndarray) -> np.ndarray:
    if not mask.any():
        return np.zeros(3)
    return img[:, :, :3][mask].mean(axis=0)


def main() -> int:
    ap = argparse.ArgumentParser(description="一次性动作帧序列对齐到默认循环画布")
    ap.add_argument("--src", required=True, help="抠像 frames 目录（原始画幅，未裁剪）")
    ap.add_argument("--out", required=True, help="输出目录")
    ap.add_argument("--ref-params", default=None,
                    help="默认循环的 抠像参数.json（自动读取 crop 与输出边长）")
    ap.add_argument("--crop-x", type=int, default=None, help="手动指定裁剪框左上 x")
    ap.add_argument("--crop-y", type=int, default=None, help="手动指定裁剪框左上 y")
    ap.add_argument("--crop-size", type=int, default=None, help="手动指定裁剪框边长")
    ap.add_argument("--size", type=int, default=0,
                    help="输出边长，0=跟随 --ref-params 的最终尺寸（默认 588）")
    ap.add_argument("--anchor", default=None,
                    help="默认循环的锚点帧（通常是 idle-combo/f0000.png），用于对齐自检")
    ap.add_argument("--action-id", default="yawn", help="动作 id")
    ap.add_argument("--frame-duration-ms", type=int, default=FRAME_MS)
    ap.add_argument("--allow-overflow", action="store_true",
                    help="显式允许运动范围超出取景框（超出的部分会被 crop 裁掉）。"
                         "仅交互动作（如 grab-release 站立必然超坐姿框）可用；"
                         "偶发动作超框应重出而非放宽。")
    args = ap.parse_args()

    src = Path(args.src)
    out = Path(args.out)
    frames_out = out / "frames"
    frames_out.mkdir(parents=True, exist_ok=True)

    # ---- 取景框：优先用默认循环的参数，保证坐标系一致 ----
    crop_x, crop_y, crop_size = args.crop_x, args.crop_y, args.crop_size
    out_size = args.size
    ref_note = "手动指定"
    if args.ref_params:
        rp = json.loads(Path(args.ref_params).read_text(encoding="utf-8"))
        c = rp["crop"]
        crop_x = int(c["x"]) if crop_x is None else crop_x
        crop_y = int(c["y"]) if crop_y is None else crop_y
        crop_size = int(c["size"]) if crop_size is None else crop_size
        if out_size <= 0:
            # 默认循环的运行时边长：抠像输出再经 帧序列 脚本缩放后的结果
            out_size = int(rp["outputSize"][0])
        ref_note = str(args.ref_params)
    assert None not in (crop_x, crop_y, crop_size), "缺少裁剪框参数"
    assert out_size > 0, "缺少输出边长"

    names = sorted(src.glob("f*.png"))
    assert names, f"源目录无帧: {src}"
    imgs = [imread_rgba(p) for p in names]
    n = len(imgs)
    h, w = imgs[0].shape[:2]
    print(f"[输入] {n} 帧 {w}x{h}  取景框 ({crop_x},{crop_y},{crop_size}) -> {out_size}x{out_size}")
    print(f"[取景] 来源 {ref_note}")

    # ---- 越界检查：新动作的运动范围必须装进默认循环的取景框 ----
    union = [10 ** 9, 10 ** 9, -1, -1]
    for im in imgs:
        b = bbox_of(fg_mask(im))
        if b is None:
            continue
        union[0] = min(union[0], b[0])
        union[1] = min(union[1], b[1])
        union[2] = max(union[2], b[2])
        union[3] = max(union[3], b[3])
    overflow = {
        "left": max(0, crop_x - union[0]),
        "top": max(0, crop_y - union[1]),
        "right": max(0, union[2] - (crop_x + crop_size - 1)),
        "bottom": max(0, union[3] - (crop_y + crop_size - 1)),
    }
    print(f"[越界] 运动范围并集 bbox={union}  余量(左/上/右/下)="
          f"{union[0]-crop_x}/{union[1]-crop_y}/"
          f"{crop_x+crop_size-1-union[2]}/{crop_y+crop_size-1-union[3]}")
    if any(v > 0 for v in overflow.values()):
        if args.allow_overflow:
            print(f"[警告] 运动范围超出默认循环取景框 {overflow}，"
                  f"--allow-overflow 已显式接受，超出部分将被 crop 裁掉")
        else:
            print(f"[硬失败] 运动范围超出默认循环取景框: {overflow}")
            print("         -> 需要放宽取景框，但那会让默认循环也变（两个动作一起重出）")
            return 1

    # ---- 裁剪 + 缩放 ----
    scale = out_size / crop_size
    out_imgs: list[np.ndarray] = []
    for im in imgs:
        c = im[crop_y:crop_y + crop_size, crop_x:crop_x + crop_size]
        if scale != 1.0:
            c = cv2.resize(c, (out_size, out_size), interpolation=cv2.INTER_AREA)
        out_imgs.append(c)
    print(f"[变换] crop({crop_x},{crop_y},{crop_size}) + resize x{scale:.6f}")

    # ---- 落盘 ----
    files = []
    frame_rels = []
    for i, im in enumerate(out_imgs):
        fp = frames_out / f"f{i:04d}.png"
        imwrite_rgba(fp, im)
        rel = f"frames/f{i:04d}.png"
        frame_rels.append(rel)
        files.append({"role": "frame", "relativePath": rel, "sha256": sha256_of(fp)})
    print(f"[输出] {n} 帧 -> {frames_out}")

    # ---- 自检 ----
    report: dict = {
        "src": str(src),
        "frameCount": n,
        "frameDurationMs": args.frame_duration_ms,
        "durationMs": n * args.frame_duration_ms,
        "crop": {"x": crop_x, "y": crop_y, "size": crop_size},
        "outputSize": out_size,
        "unionForegroundBBoxInSource": union,
        "overflow": overflow,
    }

    masks = [fg_mask(im) for im in out_imgs]
    areas = np.array([int(m.sum()) for m in masks], dtype=np.float64)
    base = areas[0]
    wave = float(np.abs(areas - base).max() / base) if base else 0.0
    report["areaBaseline"] = int(base)
    report["areaMaxWave"] = round(wave, 6)
    print(f"[自检] 面积 base={int(base)} 最大波动 {wave:.2%} "
          f"{'PASS' if wave <= AREA_WAVE_WARN else 'WARN'}")

    touched = [i for i, m in enumerate(masks)
               if m[:, :2].any() or m[:, -2:].any() or m[:2, :].any() or m[-2:, :].any()]
    report["touchEdgeFrames"] = touched
    print(f"[自检] 触边帧 {len(touched)}/{n} "
          f"{'PASS' if not touched else 'WARN ' + str(touched[:10])}")

    if args.anchor:
        anchor = imread_rgba(Path(args.anchor))
        am = fg_mask(anchor)
        ab = bbox_of(am)
        fb = bbox_of(masks[0])
        shift = [0, 0, 0, 0]
        if ab and fb:
            shift = [fb[0] - ab[0], fb[1] - ab[1], fb[2] - ab[2], fb[3] - ab[3]]
        iou0 = iou(masks[0], am)
        cdiff = float(np.abs(mean_rgb(out_imgs[0], masks[0])
                             - mean_rgb(anchor, am)).max())
        report["anchor"] = {
            "path": str(args.anchor),
            "anchorBBox": ab,
            "firstFrameBBox": fb,
            "bboxShiftLRBT": shift,
            "firstFrameIoU": round(iou0, 4),
            "colorMeanAbsDiff": round(cdiff, 3),
        }
        print(f"[自检] 锚点对齐 bbox 锚={ab} 首帧={fb} 偏移(L,R,T,B)={shift}")
        print(f"[自检] 首帧/锚点 IoU={iou0:.4f} "
              f"{'PASS' if iou0 >= IOU_WARN else 'WARN 触发瞬间可能跳变'}")
        print(f"[自检] 主体 RGB 均值最大差 {cdiff:.2f} "
              f"{'PASS' if cdiff <= COLOR_SHIFT_WARN else 'WARN 两动作颜色不一致'}")

    (out / "manifest.json").write_text(json.dumps({
        "renderer": "frame-sequence-v1",
        "actionId": args.action_id,
        "loop": False,
        "frameDurationMs": args.frame_duration_ms,
        "frameCount": n,
        "durationMs": n * args.frame_duration_ms,
        "size": out_size,
        "alignment": {
            "mode": "shared-crop-with-default-loop",
            "crop": {"x": crop_x, "y": crop_y, "size": crop_size},
            "scale": round(scale, 8),
            "sourceRef": ref_note,
        },
        "files": files,
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    (out / "对齐自检.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"[输出] {out / 'manifest.json'}")
    print(f"[输出] {out / '对齐自检.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

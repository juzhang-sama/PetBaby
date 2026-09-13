# -*- coding: utf-8 -*-
"""抠像后的帧序列四项机械验收。

从 `scripts/poc_验收.py` **原样搬进服务**（判据与阈值零改动）：脚本现在只是这里的薄 CLI 包装。

通过标准（用户定义，**不得自行放宽**）：
    1. 尾巴完整
    2. 胡须和耳缘无明显绿边
    3. 帧间不闪烁
    4. 在黑白棋盘格背景上都自然

产物：`验收报告.json` + 五张证据图（棋盘格 / 黑底 / 白底 / 边缘放大 / 闪烁热力图）。

⚠️ **机械指标只能排雷，肉眼判定 = 权威。** 指标 FAIL 但证据图干净 = 误报，继续走。
   判真伪只看三个真实缺陷信号：飞块 / 触边 / 多连通块。面积波动判据不适用于全身/头颈动。
❌ 任一 FAIL 时的约定动作是**先换抠像方法**（`--method rvm` / `sam2`），
   不扩展动作、不换宠物 —— 不要把阈值放宽。

⚠️ 内存：`load_frames` 会把整段帧序列读成 `uint8 RGBA` + `float32 alpha` 两份
（288 帧 @614² ≈ 870MB 峰值）。288 帧是本项目当前的规模上限，可以接受；
若将来帧数上量，改成逐帧流式读取。
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import numpy as np
from PIL import Image
from scipy import ndimage

from ._paths import rel_to

# ---------------------------------------------------------------- 阈值
# 说明：阈值以自检样本（理想绿幕）为基准留出余量，属于 provisional，
#       真实视频第一轮跑完后应据实测分布回看是否过松/过紧。
THRESHOLDS = {
    # 1 尾巴完整
    "strayComponentAreaRatio": 0.0002,   # 非主体连通块面积上限（占画布）
    "foregroundAreaVariation": 0.06,     # 前景面积最大波动 / 中位面积
    # 2 绿边
    "greenFringeRatio": 0.02,             # 边缘像素中偏绿像素占比上限
    "greenExcess": 18.0,                  # G - max(R,B) 超过该值判为偏绿
    "interiorSpillRatio": 0.005,          # 主体内部（alpha>0.9）偏绿像素占比上限
    # 3 闪烁（用空间聚合量，逐像素指标会被运动污染，只作诊断）
    "silhouetteAreaJitter": 0.005,        # 轮廓面积时间序列的二阶差分（归一化）上限
    "alphaMassJitter": 0.005,             # alpha 总量时间序列的二阶差分（归一化）上限
    "interiorAlphaStd": 0.01,             # 主体内部 alpha 的时间标准差上限
    # 4 合成自然度（黑/白/棋盘格背景都是消色，用彩度判定污染）
    "edgeChroma": 12.0,                   # 外圈合成结果平均彩度上限（0-255）
    "edgeGreenExcess": 6.0,               # 外圈 G-max(R,B) 平均上限（0-255）
    "ringMeanAlpha": 0.30,                # 外圈平均 alpha 上限，过大 = 边缘发虚
}

# 四项判据的展示名（同时是 `验收报告.json` 里 `criteria` 的键，别改名）
CRITERIA = ("1-尾巴完整", "2-无绿边", "3-帧间不闪烁", "4-黑白棋盘格自然")

EVIDENCE_LABELS = {"checker": "棋盘格", "black": "黑底", "white": "白底"}


class AcceptanceError(ValueError):
    """验收的失败态。必须是 `Exception` 子类 —— 见 `frames/__init__.py` 的约定。"""


def load_frames(frames_dir: Path) -> tuple[np.ndarray, np.ndarray]:
    """返回 (rgba_stack[N,H,W,4] uint8, alpha_stack[N,H,W] float32 0-1)"""
    files = sorted(Path(frames_dir).glob("f*.png"))
    if not files:
        raise AcceptanceError(f"[缺输入] 没有帧文件: {frames_dir}")
    stack = np.stack([np.array(Image.open(p).convert("RGBA")) for p in files])
    alphas = stack[:, :, :, 3].astype(np.float32) / 255.0
    return stack, alphas


def check_tail(alphas: np.ndarray) -> dict:
    """尾巴完整：不出现断裂飞块、面积不突变、没有被取景框裁到。

    注意：不能用「包围盒右边界的变化幅度」来判断尾巴完不完整。
    摇尾巴时尾尖本来就会左右大幅摆动，那个量测的是**动作幅度**，
    幅度大恰恰说明尾巴在动，不是尾巴没了。真实的不完整只有三种表现：
      ① 断裂成飞块  ② 面积突然塌陷  ③ 主体贴到画布边（被裁切）
    """
    mask = alphas >= 0.5
    h, w = mask.shape[1], mask.shape[2]
    canvas = float(h * w)

    stray_frames, max_components, areas, rights, touch_frames = [], 0, [], [], []
    for i, m in enumerate(mask):
        labeled, count = ndimage.label(m)
        max_components = max(max_components, count)
        if count > 1:
            sizes = ndimage.sum(m, labeled, range(1, count + 1))
            main = int(np.argmax(sizes)) + 1
            stray = max([s for k, s in enumerate(sizes, start=1) if k != main] or [0.0])
            if stray / canvas > THRESHOLDS["strayComponentAreaRatio"]:
                stray_frames.append({"frame": i, "strayAreaRatio": round(float(stray / canvas), 6)})
        areas.append(float(m.sum()))
        ys, xs = np.nonzero(m)
        if xs.size:
            rights.append(int(xs.max()))
            # 贴边 = 被取景框裁切（留 2px 容差）
            if xs.min() <= 1 or ys.min() <= 1 or xs.max() >= w - 2 or ys.max() >= h - 2:
                touch_frames.append({"frame": i,
                                     "bbox": [int(xs.min()), int(ys.min()),
                                              int(xs.max()), int(ys.max())]})
        else:
            rights.append(0)

    areas = np.array(areas)
    median_area = float(np.median(areas))
    area_var = float((areas.max() - areas.min()) / median_area) if median_area else 0.0

    rights = np.array(rights)
    # 仅作诊断：尾尖摆动幅度（这是动作幅度，不是缺陷）
    swing = float((rights.max() - rights.min()) / w) if w else 0.0

    passed = (not stray_frames) and area_var <= THRESHOLDS["foregroundAreaVariation"] \
        and not touch_frames
    return {
        "passed": bool(passed),
        "strayComponentFrames": stray_frames[:10],
        "strayComponentFrameCount": len(stray_frames),
        "edgeTouchFrames": touch_frames[:10],
        "edgeTouchFrameCount": len(touch_frames),
        "maxConnectedComponents": int(max_components),
        "foregroundAreaVariation": round(area_var, 5),
        "diagnostic_tailSwingAmplitude": round(swing, 5),
        "thresholds": {
            "strayComponentAreaRatio": THRESHOLDS["strayComponentAreaRatio"],
            "foregroundAreaVariation": THRESHOLDS["foregroundAreaVariation"],
        },
        "note": "strayComponentFrames 非空说明尾巴被抠成飞块；edgeTouchFrames 非空说明"
                "主体贴到画布边、被取景框裁掉；diagnostic_tailSwingAmplitude 是尾尖摆动"
                "幅度，属于正常动作，不参与判定。",
    }


def check_green_fringe(stack: np.ndarray, alphas: np.ndarray) -> dict:
    """胡须和耳缘无明显绿边：检查边缘环与半透明带的 G-max(R,B)。"""
    rgb = stack[:, :, :, :3].astype(np.float32)
    g = rgb[:, :, :, 1]
    rb = np.maximum(rgb[:, :, :, 0], rgb[:, :, :, 2])
    excess = g - rb

    mask = alphas >= 0.5
    struct = np.ones((3, 3), dtype=bool)

    fringe_ratios, interior_ratios = [], []
    for i in range(alphas.shape[0]):
        m = mask[i]
        # 边缘环：主体外沿 1-2px，对应耳缘、胡须、毛尖
        eroded = ndimage.binary_erosion(m, structure=struct)
        boundary = m & (~eroded)
        # 半透明抗锯齿带
        aa = (alphas[i] > 0.05) & (alphas[i] < 0.95)
        edge_zone = boundary | aa
        if edge_zone.sum():
            fringe_ratios.append(float(((excess[i] > THRESHOLDS["greenExcess"]) & edge_zone).sum()
                                       / edge_zone.sum()))
        else:
            fringe_ratios.append(0.0)
        interior = alphas[i] > 0.9
        if interior.sum():
            interior_ratios.append(float(((excess[i] > THRESHOLDS["greenExcess"]) & interior).sum()
                                         / interior.sum()))
        else:
            interior_ratios.append(0.0)

    fringe = float(np.mean(fringe_ratios))
    fringe_max = float(np.max(fringe_ratios))
    interior = float(np.mean(interior_ratios))
    passed = fringe <= THRESHOLDS["greenFringeRatio"] and interior <= THRESHOLDS["interiorSpillRatio"]
    return {
        "passed": bool(passed),
        "edgeGreenFringeRatioMean": round(fringe, 6),
        "edgeGreenFringeRatioMax": round(fringe_max, 6),
        "interiorGreenSpillRatio": round(interior, 6),
        "thresholds": {
            "greenFringeRatio": THRESHOLDS["greenFringeRatio"],
            "greenExcess": THRESHOLDS["greenExcess"],
            "interiorSpillRatio": THRESHOLDS["interiorSpillRatio"],
        },
        "note": "edgeGreenFringeRatio 统计耳缘/胡须/毛尖所在边缘环与抗锯齿带的偏绿像素；"
                "interiorGreenSpillRatio 统计主体内部的绿色反光残留。",
    }


def check_flicker(stack: np.ndarray, alphas: np.ndarray) -> dict:
    """帧间不闪烁。

    判据设计的关键教训（踩过两次坑，别再改回去）：

    1. 不能用「单个像素的 alpha 时间变化」来判定闪烁。
       边缘以 3px/帧扫过某个像素时，该像素一帧内 0→1，二阶差分就是 1.0，
       但人眼看到的是**运动**。实测真实摇尾视频这一项高达 0.14，
       而干净的静态基准也有 0.16 —— 完全无法区分，是假警报。

    2. 正确做法是用**空间聚合量**：把整帧的前景面积/alpha 总量排成时间序列，
       平滑运动给出平滑的曲线，只有真正的抖动才会让曲线变锯齿。
       单像素噪声在求和时相互抵消，边缘扫过也不会污染总量。

    3. 再补一个「主体内部稳定性」：内部像素的 alpha 恒为 1，
       任何时间波动都只可能来自抠像噪声。
    """
    n = alphas.shape[0]
    if n < 3:
        return {"passed": False, "error": "帧数不足，至少需要 3 帧"}

    # ---- 主判据 1：轮廓面积时间序列的平滑度（空间聚合，对运动免疫）----
    area = (alphas > 0.5).sum(axis=(1, 2)).astype(np.float64)
    area_jitter = float(np.abs(area[2:] - 2 * area[1:-1] + area[:-2]).mean() / max(1e-9, area.mean()))

    # ---- 主判据 2：alpha 总量的平滑度 ----
    mass = alphas.sum(axis=(1, 2)).astype(np.float64)
    mass_jitter = float(np.abs(mass[2:] - 2 * mass[1:-1] + mass[:-2]).mean() / max(1e-9, mass.mean()))

    # ---- 主判据 3：主体内部稳定性（alpha>0.95 的像素不该有任何时间波动）----
    mean_a = alphas.mean(axis=0)
    inner = mean_a > 0.95
    if inner.sum():
        inner_std = float(alphas[:, inner].std(axis=0).mean())
    else:
        inner_std = 0.0

    # ---- 诊断量：逐像素指标（受真实运动污染，不参与判定）----
    a2 = np.abs(alphas[2:] - 2 * alphas[1:-1] + alphas[:-2])
    edge_band = (alphas[1:-1] > 0.02) & (alphas[1:-1] < 0.98)
    per_pixel_2nd = float((a2[edge_band] > 0.25).mean()) if edge_band.sum() else 0.0
    d_alpha = np.abs(np.diff(alphas, axis=0))
    rgb = stack[:, :, :, :3].astype(np.float32)
    r2 = np.abs(rgb[2:] - 2 * rgb[1:-1] + rgb[:-2]).max(axis=3)
    both_fg = alphas[1:-1] > 0.9
    rgb_2nd = float((r2[both_fg] > 40).mean()) if both_fg.sum() else 0.0

    passed = (area_jitter <= THRESHOLDS["silhouetteAreaJitter"] and
              mass_jitter <= THRESHOLDS["alphaMassJitter"] and
              inner_std <= THRESHOLDS["interiorAlphaStd"])
    return {
        "passed": bool(passed),
        "silhouetteAreaJitter": round(area_jitter, 6),
        "alphaMassJitter": round(mass_jitter, 6),
        "interiorAlphaStd": round(inner_std, 6),
        "diagnostic_perPixelSecondDiffRatio": round(per_pixel_2nd, 6),
        "diagnostic_rgbSecondDiffRatio": round(rgb_2nd, 6),
        "diagnostic_maxAlphaJumpPerFrame": round(float(d_alpha.max()), 4),
        "thresholds": {
            "silhouetteAreaJitter": THRESHOLDS["silhouetteAreaJitter"],
            "alphaMassJitter": THRESHOLDS["alphaMassJitter"],
            "interiorAlphaStd": THRESHOLDS["interiorAlphaStd"],
        },
        "note": "主判据是空间聚合量：silhouetteAreaJitter / alphaMassJitter 是轮廓面积与"
                "alpha 总量时间序列的二阶差分（平滑运动≈0，抖动会变锯齿），"
                "interiorAlphaStd 是主体内部 alpha 的时间标准差（应接近 0）。"
                "diagnostic_* 是逐像素指标，会被尾巴扫过等真实运动污染，不参与判定。",
    }


def make_background(size: tuple[int, int], kind: str) -> np.ndarray:
    h, w = size
    if kind == "black":
        bg = np.zeros((h, w, 3), np.float32)
    elif kind == "white":
        bg = np.full((h, w, 3), 255.0, np.float32)
    else:  # checker
        cell = max(8, h // 32)
        yy, xx = np.mgrid[0:h, 0:w]
        chk = (((xx // cell) + (yy // cell)) % 2) == 0
        bg = np.where(chk[..., None], 238.0, 198.0).astype(np.float32)
        bg = np.repeat(bg, 3, axis=2).astype(np.float32)
    return bg


def composite(stack: np.ndarray, alphas: np.ndarray, kind: str) -> np.ndarray:
    bg = make_background(alphas.shape[1:3], kind)
    out = bg[None, ...] * (1.0 - alphas[..., None]) + \
        stack[:, :, :, :3].astype(np.float32) * alphas[..., None]
    return np.clip(out, 0, 255).astype(np.uint8)


def check_halo(stack: np.ndarray, alphas: np.ndarray) -> dict:
    """合成自然度：主体外圈不应出现与背景不符的光晕。"""
    mask = alphas >= 0.5
    if not mask.any():
        return {"passed": False, "error": "没有前景"}
    # 用首帧计算外圈位置；本 POC 构图基本不变，可近似复用
    dist = ndimage.distance_transform_edt(~mask[0])
    ring = (dist > 0) & (dist <= 3)
    if not ring.any():
        return {"passed": False, "error": "无法定位主体外圈"}

    # 关键：黑/白/棋盘格三种背景都是消色的（R=G=B）。
    # 因此合成图外圈一旦出现「彩度」，就只可能来自抠像污染（绿边/去绿过头），
    # 而不是抗锯齿本身——抗锯齿只改变亮度，不改变彩度。
    chroma_by_bg, green_by_bg, ring_alpha_by_bg = {}, {}, {}
    for kind in ("black", "white", "checker"):
        bg = make_background(alphas.shape[1:3], kind)
        chromas, greens, ring_as = [], [], []
        for i in range(alphas.shape[0]):
            ring_i = ring & (~mask[i])
            if not ring_i.any():
                continue
            a = alphas[i][..., None]
            comp = bg * (1.0 - a) + stack[i][:, :, :3].astype(np.float32) * a
            comp = np.clip(comp, 0, 255)
            chroma = comp.max(axis=2) - comp.min(axis=2)
            gexcess = comp[:, :, 1] - np.maximum(comp[:, :, 0], comp[:, :, 2])
            chromas.append(float(chroma[ring_i].mean()))
            greens.append(float(np.clip(gexcess, 0, None)[ring_i].mean()))
            ring_as.append(float(alphas[i][ring_i].mean()))
        chroma_by_bg[kind] = round(float(np.mean(chromas)), 4) if chromas else 0.0
        green_by_bg[kind] = round(float(np.mean(greens)), 4) if greens else 0.0
        ring_alpha_by_bg[kind] = round(float(np.mean(ring_as)), 4) if ring_as else 0.0

    worst_chroma = max(chroma_by_bg.values())
    worst_green = max(green_by_bg.values())
    worst_ring_alpha = max(ring_alpha_by_bg.values())
    passed = (worst_chroma <= THRESHOLDS["edgeChroma"] and
              worst_green <= THRESHOLDS["edgeGreenExcess"] and
              worst_ring_alpha <= THRESHOLDS["ringMeanAlpha"])
    return {
        "passed": bool(passed),
        "edgeChromaByBackground": chroma_by_bg,
        "edgeGreenExcessByBackground": green_by_bg,
        "ringMeanAlphaByBackground": ring_alpha_by_bg,
        "worstEdgeChroma": round(worst_chroma, 4),
        "worstEdgeGreenExcess": round(worst_green, 4),
        "worstRingMeanAlpha": round(worst_ring_alpha, 4),
        "thresholds": {
            "edgeChroma": THRESHOLDS["edgeChroma"],
            "edgeGreenExcess": THRESHOLDS["edgeGreenExcess"],
            "ringMeanAlpha": THRESHOLDS["ringMeanAlpha"],
        },
        "note": "黑/白/棋盘格背景均为消色，外圈出现彩度即代表抠像污染；"
                "ringMeanAlpha 过大说明边缘发虚、存在半透明脏边。",
    }


def save_evidence(
    stack: np.ndarray,
    alphas: np.ndarray,
    out_dir: Path,
    *,
    path_base: Path | None = None,
) -> dict:
    """拼证据图：三张背景对照 + 边缘放大 + 闪烁热力图。返回 {键: 相对路径}。"""
    out_dir = Path(out_dir)
    n = alphas.shape[0]
    picks = np.linspace(0, n - 1, min(9, n)).astype(int)

    sheets = {}
    for kind in ("checker", "black", "white"):
        comp = composite(stack[picks], alphas[picks], kind)
        h, w = comp.shape[1], comp.shape[2]
        scale = 3
        cols = 3
        rows = int(np.ceil(len(picks) / cols))
        sheet = Image.new("RGB", (w // scale * cols, h // scale * rows), (128, 128, 128))
        for idx, frame in enumerate(comp):
            img = Image.fromarray(frame).resize((w // scale, h // scale), Image.LANCZOS)
            sheet.paste(img, ((idx % cols) * (w // scale), (idx // cols) * (h // scale)))
        path = out_dir / f"证据-{EVIDENCE_LABELS[kind]}.png"
        sheet.save(path)
        sheets[kind] = rel_to(path, path_base)

    # 边缘放大：取耳缘/胡须候选区域（细结构最密集处）
    mask0 = alphas[0] >= 0.5
    eroded = ndimage.binary_erosion(mask0, structure=np.ones((3, 3), bool))
    thin = mask0 & (~eroded)
    ys, xs = np.nonzero(thin)
    crops = []
    if ys.size:
        # 选细结构最密的 64x64 区块
        h, w = mask0.shape
        grid = 64
        counts = np.zeros((h // grid + 1, w // grid + 1), int)
        np.add.at(counts, (ys // grid, xs // grid), 1)
        gy, gx = np.unravel_index(np.argmax(counts), counts.shape)
        y0, x0 = max(0, gy * grid - 32), max(0, gx * grid - 32)
        y1, x1 = min(h, y0 + 128), min(w, x0 + 128)
        for kind in ("checker", "black", "white"):
            comp = composite(stack[[0]], alphas[[0]], kind)[0]
            crop = Image.fromarray(comp[y0:y1, x0:x1]).resize(
                ((x1 - x0) * 4, (y1 - y0) * 4), Image.NEAREST)
            crops.append(crop)
    if crops:
        total_w = sum(c.width for c in crops)
        sheet = Image.new("RGB", (total_w, crops[0].height), (128, 128, 128))
        x = 0
        for c in crops:
            sheet.paste(c, (x, 0))
            x += c.width
        path = out_dir / "证据-边缘放大.png"
        sheet.save(path)
        sheets["edgeZoom"] = rel_to(path, path_base)

    # 闪烁热力图：时间维度上的最大 alpha 跳变
    d = np.abs(np.diff(alphas, axis=0)).max(axis=0) if n >= 2 else np.zeros(alphas.shape[1:])
    if d.max() > 0:
        norm = (np.clip(d / 0.3, 0, 1) * 255).astype(np.uint8)
    else:
        norm = np.zeros(alphas.shape[1:], np.uint8)
    heat = Image.fromarray(norm).resize((512, 512), Image.NEAREST)
    path = out_dir / "证据-闪烁热力图.png"
    heat.save(path)
    sheets["flickerHeatmap"] = rel_to(path, path_base)
    return sheets


@dataclass(frozen=True)
class AcceptanceResult:
    out_dir: Path
    report_path: Path
    report: dict
    overall_passed: bool
    frame_count: int
    resolution: tuple[int, int]
    evidence: dict

    @property
    def criteria(self) -> dict:
        return self.report["criteria"]

    @property
    def failed_criteria(self) -> tuple[str, ...]:
        """FAIL 的判据名。全 PASS 时为空元组。

        调用方据此决定要不要换抠像方法 —— **不许放宽阈值**。
        """
        return tuple(name for name, result in self.criteria.items() if not result.get("passed"))


def accept_frames(
    frames_dir: Path,
    out_dir: Path,
    *,
    path_base: Path | None = None,
    log: Callable[[str], None] = print,
) -> AcceptanceResult:
    """对 `frames_dir` 下的 `f*.png` 跑四项判据并写出报告与证据图。

    **返回结果不代表通过**：FAIL 也正常产出报告（判据数据要留档），
    是否放行由调用方看 `overall_passed` / `failed_criteria` 决定。
    旧 CLI 的约定是 rc=2 表示「判定不通过」而非崩溃，照旧。
    """
    frames_dir = Path(frames_dir)
    out_dir = Path(out_dir)
    if not frames_dir.is_dir():
        raise AcceptanceError(
            f"[缺输入] 帧序列目录不存在: {frames_dir}（先跑抠像）"
        )
    out_dir.mkdir(parents=True, exist_ok=True)

    stack, alphas = load_frames(frames_dir)
    log(f"[输入] {frames_dir}  共 {alphas.shape[0]} 帧  {alphas.shape[2]}x{alphas.shape[1]}")

    report = {
        "framesDir": rel_to(frames_dir, path_base),
        "frameCount": int(alphas.shape[0]),
        "resolution": [int(alphas.shape[2]), int(alphas.shape[1])],
        "criteria": {},
    }

    checks = [
        (CRITERIA[0], check_tail(alphas)),
        (CRITERIA[1], check_green_fringe(stack, alphas)),
        (CRITERIA[2], check_flicker(stack, alphas)),
        (CRITERIA[3], check_halo(stack, alphas)),
    ]
    for name, result in checks:
        report["criteria"][name] = result
        log(f"[{'PASS' if result.get('passed') else 'FAIL'}] {name}")

    report["overallPassed"] = all(r.get("passed") for _, r in checks)
    report["evidence"] = save_evidence(stack, alphas, out_dir, path_base=path_base)

    report_path = out_dir / "验收报告.json"
    report_path.write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    return AcceptanceResult(
        out_dir=out_dir,
        report_path=report_path,
        report=report,
        overall_passed=bool(report["overallPassed"]),
        frame_count=int(alphas.shape[0]),
        resolution=(int(alphas.shape[2]), int(alphas.shape[1])),
        evidence=report["evidence"],
    )

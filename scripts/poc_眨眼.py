# -*- coding: utf-8 -*-
"""程序化合成眨眼动作帧序列。

为什么是"合成"而不是"AI 生成整段视频"：
  眨眼是**局部、形态明确、时长极短**的动作。整段 AI 视频会重画全身，
  带来两个问题：①呼吸被打断时身体姿态跳变；②每次生成眼睛形态不一致。
  合成方案只替换眼部，身体直接复用呼吸循环的锚点帧，跳变为 0。

代价：需要一次 gpt-image-2 图生图调用，产出"只闭眼、其他不变"的图。
这一步有失败风险（模型可能改姿态），所以脚本会先做对齐校验再合成。

流程：
  闭眼图(2048) --缩放--> 1024 --对齐--> 母版-1024 --(母版→锚点变换)--> 588
  588 尺度上：检测眼球(黄色) -> 取外扩 patch -> 羽化 -> 贴回锚点帧

用法：
  D:/DevTools/Python312/python.exe scripts/poc_眨眼.py \
      --closed output/.../06-眨眼/00-闭眼版/母版-XXXX.png \
      --master output/.../01-首帧/母版-1024.png \
      --anchor output/.../04-帧序列-呼吸循环/frames/f0000.png \
      --out    output/.../06-眨眼/01-帧序列
"""
from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]

# 眼周外扩（替换矩形在眼球 bbox 基础上再外扩的像素）
EYE_PAD = 16
# 羽化半径：patch 边缘渐隐，避免硬边接缝
FEATHER = 9
# 眨眼序列：眼睑闭合程度 p（0=全睁，1=全闭）。
# 首尾必须是 0 —— 动作单元首尾帧要能直接接回呼吸锚点帧，否则插入/退出时眼睛会突跳。
#
# 【定版值，2026-09-02 老王实拍选定】12 帧 × 42ms = 504ms
#   - 老王最初嫌"眨眼太快"，原 4 帧 168ms 的睁相只有 1 帧（0.45→0 直接跳锚点），像"啪"一下
#   - 12 帧版闭:睁 = 2:5 ≈ 1:2.5，接近生理比例；末帧留 0 保证回锚点零跳变
#   - 老王在三版（blend 70% 叠加态 / wipe 2% / aperture 7%）中**选了 blend**：
#     在 504ms 慢速下，blend 的渐变从"拖影"变成"柔和过渡"，眼周仅占画布 2.7% 肉眼分辨不出加权
# 换序列时同步改 `frame-sequence-merged-package.test.ts` 里的 BLINK_FRAMES / BLINK_MS。
CLOSE_SEQUENCE = [0.0, 0.3, 0.6, 0.85, 1.0, 1.0, 0.85, 0.6, 0.3, 0.1, 0.05, 0.0]
# 对齐细化搜索半径（像素）
REFINE_RADIUS = 8
# 眼睑扫描（wipe 模式）参数
# 弧度给的是"占眼球高度的比例"而不是绝对像素：588 画布上这只猫的眼睛只有 17px 高，
# 绝对像素值换个宠物/换个分辨率就完全不对了。
LID_CURVE_FRAC = 0.15  # 眼睑线向下凸：中间比两边低 眼球高度 × 此比例
LID_FEATHER = 1        # 眼睑边缘过渡带宽度（像素）—— 唯一允许出现"中间态"的地方
                      # 588 画布上眼睛只有 17px 高，feather=2 时 2px 眼缝整个都是过渡带
                      # （叠加态虚高到 13%）；feather=1 降到 7%，像素画也更贴合硬边
LID_MARGIN = 2         # 扫描范围在眼球 bbox 上下各外扩多少像素
# 眼缝收拢（aperture 模式）参数
# 上眼睑贡献的闭合比例：真实眨眼上眼睑动得多、下眼睑几乎不动（上 ~70% / 下 ~30%）
APERTURE_UP_FRAC = 0.70
# 残影自检：|closed-open| 小于此值的像素不参与反解（两图几乎相同，反解无意义）
GHOST_MIN_DENOM = 24.0
GHOST_LO, GHOST_HI = 0.20, 0.80


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


def alpha_mask(img: np.ndarray, thresh: int = 128) -> np.ndarray:
    return img[:, :, 3] > thresh


def bbox_of(mask: np.ndarray) -> tuple[int, int, int, int]:
    ys, xs = np.nonzero(mask)
    if ys.size == 0:
        raise SystemExit("[错误] 掩码为空，找不到主体")
    return int(ys.min()), int(ys.max()), int(xs.min()), int(xs.max())


def refine_shift(src: np.ndarray, dst: np.ndarray, dx: int, dy: int, radius: int) -> tuple[int, int]:
    """在 init 偏移附近搜索使两个 alpha 掩码最贴合的平移（最小化 XOR）。"""
    best = (dx, dy, None)
    for oy in range(-radius, radius + 1):
        for ox in range(-radius, radius + 1):
            shifted = np.roll(np.roll(src, (dy + oy), axis=0), (dx + ox), axis=1)
            cost = int(np.count_nonzero(shifted ^ dst))
            if best[2] is None or cost < best[2]:
                best = (dx + ox, dy + oy, cost)
    return best[0], best[1], best[2]


def detect_eyes(anchor: np.ndarray) -> tuple[tuple[int, int, int, int],
                                              list[tuple[int, int, int, int]]]:
    """用黄色眼球定位眼睛：猫的虹膜是高饱和暖黄，与毛色区分明显。

    返回 (合并 bbox, 每只眼睛的 bbox 列表)，bbox 格式均为 (y0, y1, x0, x1)。
    每只眼睛单独返回是为了眼睑扫描 —— 两只眼睛要各自算一条弧，
    用一条横跨整个 patch 的弧会让每只眼睛只吃到半边弧，形状不对。
    """
    a = anchor[:, :, 3]
    rgb = anchor[:, :, :3]
    hsv = cv2.cvtColor(rgb, cv2.COLOR_BGR2HSV)
    h = hsv[:, :, 0].astype(np.int16)
    s = hsv[:, :, 1].astype(np.int16)
    v = hsv[:, :, 2].astype(np.int16)
    mask = ((h >= 18) & (h <= 45) & (s >= 100) & (v >= 120) & (a > 128)).astype(np.uint8) * 255

    n, _labels, stats, _cent = cv2.connectedComponentsWithStats(mask, 8)
    if n < 2:
        raise SystemExit("[错误] 没能检测到两只眼睛，请检查锚点帧或调整 HSV 阈值")
    comps = sorted(
        ((int(stats[i][4]), int(stats[i][1]), int(stats[i][0]),
          int(stats[i][3]), int(stats[i][2])) for i in range(1, n)),
        reverse=True,
    )[:2]
    eyes = [(int(c[1]), int(c[1] + c[3]), int(c[2]), int(c[2] + c[4])) for c in comps]
    eyes.sort(key=lambda e: e[2])  # 按 x 排序：左眼在前
    ys0 = min(e[0] for e in eyes)
    ys1 = max(e[1] for e in eyes)
    xs0 = min(e[2] for e in eyes)
    xs1 = max(e[3] for e in eyes)
    return (ys0, ys1, xs0, xs1), eyes


def build_feather(ph: int, pw: int, feather: int) -> np.ndarray:
    """矩形 alpha 遮罩，四边 feather 像素内线性渐隐。"""
    m = np.ones((ph, pw), np.float32)
    if feather <= 0:
        return m
    f = min(feather, ph // 2, pw // 2)
    ramp = np.linspace(0.0, 1.0, f, dtype=np.float32)
    m[:f, :] *= ramp[:, None]
    m[-f:, :] *= ramp[::-1][:, None]
    m[:, :f] *= ramp[None, :]
    m[:, -f:] *= ramp[None, ::-1]
    return m


def build_lid_mask(ph: int, pw: int, eyes_local: list[tuple[int, int, int, int]],
                   p: float, feather: int, curve_frac: float, margin: int) -> np.ndarray:
    """物理眼睑扫描遮罩：p=0 全睁，p=1 全闭。

    **为什么要这个，而不是线性混合**

    线性混合 `mid = open*(1-m) + closed*m` 在中间 m 上会同时画出睁和闭两个状态 ——
    肉眼看到的就是"眼睛已经闭上了，但睁眼的样子还残留着"，即残影。
    实测：4 帧版的中间帧有 77% 的眼周像素卡在"既不是睁也不是闭"的中间态。

    真实眨眼是**上眼睑从上往下扫过眼球**：任何一瞬间每个像素要么还睁着、
    要么已经被盖住，不存在中间态。唯一允许过渡的是眼睑边缘本身那几个像素 ——
    那是物理存在的软边，不是叠加。

    实现上是"一条有上下边界的带子"：
      上边界固定在眼球顶部（额头不受影响）
      下边界 = 眼睑线 y_lid，随 p 下移
      带子内部 = 1（闭眼状态），外部 = 0（睁眼状态）

    **p=0 时的硬约束**：眼睑线必须**整条都在眼球顶之上**（含弧度也压不下来），
    否则额头会出现"被盖了 0.5 层"的残影。基线预先减掉 max_arc 即可。
    同样地，p=1 时眼睑线必须**整条都在眼球底之下**（含弧度也提不上去）。
    """
    m = np.zeros((ph, pw), np.float32)
    ys = np.arange(ph, dtype=np.float32)[:, None]
    f = max(1.0, float(feather))   # 防止 feather=0 时除零
    for (ey0, ey1, ex0, ex1) in eyes_local:
        ex0 = max(0, int(ex0))
        ex1 = min(pw, int(ex1))
        if ex1 <= ex0:
            continue
        w = ex1 - ex0
        h = max(1, ey1 - ey0)
        top = float(ey0) - margin
        bot = float(ey1) + margin
        max_arc = curve_frac * h
        # 眼睑线：p=0 时最高点（含弧度也 ≤ top - feather）；p=1 时最低点 ≥ bot + feather
        y0 = top - f - max_arc
        y1 = bot + f
        r = (np.arange(ex0, ex1, dtype=np.float32) - (ex0 + w / 2.0)) / max(1e-6, w / 2.0)
        arc = max_arc * (1.0 - r * r)            # 向下凸，中心最大 max_arc
        y_lid = y0 + p * (y1 - y0) + arc
        # 关键：upper 和 lower 在边界处必须是 0（不是 0.5），否则 p=0 仍有泄漏
        upper = np.clip((ys - top) / f, 0.0, 1.0)                       # (ph,1)
        lower = np.clip(0.5 + (y_lid[None, :] - ys) / f, 0.0, 1.0)     # (ph,ew)
        m[:, ex0:ex1] = np.maximum(m[:, ex0:ex1], upper * lower)
    return m[:, :, None]   # 与 blend_mask 的 (h,w,1) 对齐


def build_aperture_mask(ph: int, pw: int, eyes_local: list[tuple[int, int, int, int]],
                        p: float, feather: int, curve_frac: float,
                        up_frac: float) -> np.ndarray:
    """眼缝收拢遮罩：p=0 全睁，p=1 全闭。返回的是「露出眼球」的程度（1=露，0=被盖住）。

    **为什么这个才对，wipe 和 blend 都不对**

    - **blend（线性混合）**：`mid = open*(1-m) + closed*m`，中间帧睁闭两态**同时可见**
      → 老王看到的"残影"
    - **wipe（上眼睑从上往下扫）**：中间帧是"眼睛上半闭、下半还睁"，
      看着像"半只眼"。老王一眼看出不对：**真实眨眼是眼缝变窄，不是帘子往下放**
    - **aperture（上下眼睑向中间收拢）**：眼球本身不变形，只是被上下眼皮
      遮挡的部分越来越多，露出的眼缝越来越窄。**这才是真实眨眼**

    实现：把每只眼睛拆成三块，各自从不同的源图取像素，**不做任何加权平均**：

        上眼皮区 [ey0, y_up]      ← 闭眼图（毛色眼皮）
        眼缝区   [y_up, y_dn]     ← 睁眼图（眼球，原位置原样，只是露得少）
        下眼皮区 [y_dn, ey1]      ← 闭眼图（毛色眼皮）

    上眼睑下移 / 下眼睑上移，两者在 p=1 时相遇于闭合线 yc_closed。
    上眼睑贡献 up_frac 的闭合量（真实眨眼上眼睑动得多）。

    **两端的硬约束（同 wipe 的坑，别再踩）**：
    - p=0：即使加上弧度，上下眼睑线也必须**完全在眼球之外**，否则额头/脸颊被盖半层
      → 基线预减 max_arc
    - p=1：上下眼睑线必须**互相越过 ≥ 2*feather**，否则眼睛左右两端（arc=0 处）
      会留一条 aperture=0.5 的缝 → 终点加/减 feather
    """
    slits: list[float] = []
    # 初始化为 1（= 默认露出睁眼图）而不是 0：
    # aperture 只在「眼睛 bbox 内且被眼皮盖住」的地方才是 0，
    # 眼睛之外（额头/脸颊/patch 边缘）必须保持 1，否则整块 patch 会被换成闭眼图。
    ap = np.ones((ph, pw), np.float32)
    ys = np.arange(ph, dtype=np.float32)[:, None]
    f = max(1.0, float(feather))
    for (ey0, ey1, ex0, ex1) in eyes_local:
        ex0 = max(0, int(ex0))
        ex1 = min(pw, int(ex1))
        if ex1 <= ex0:
            continue
        h = max(1.0, float(ey1 - ey0))
        w = ex1 - ex0
        # 【p 的语义 = 眼缝收窄比例，必须精确】
        # 老王要求"第一帧合并 20%，第二帧合并 40%"——p 就得直接对应眼缝高度。
        # 第一版让「安全外扩量」参与线性插值，结果 p=0.3 时虹膜只少了 8.6%（理论 30%），
        # p 的语义被外扩稀释了。这里改成：先按 p 精确算出上下眼睑位置，
        # 再把安全外扩作为**随 p 变号**的附加量加上（外扩不参与插值）。
        yc = float(ey0) + up_frac * h              # 闭合线（上眼睑贡献 up_frac）
        h_p = h * (1.0 - p)                        # 目标眼缝高度
        y_up = yc - h_p * up_frac                  # 上眼睑位置：ey0 → yc
        y_dn = yc + h_p * (1.0 - up_frac)          # 下眼睑位置：ey1 → yc
        # 眼睑边缘弧度：上眼睑中间更低、下眼睑中间更高（两条弧向中间凸）
        r = (np.arange(ex0, ex1, dtype=np.float32) - (ex0 + w / 2.0)) / max(1e-6, w / 2.0)
        arc = curve_frac * h * (1.0 - r * r)
        # 安全外扩：随 p 变号
        #   p=0 时往「外」推（眼睑线完全离开眼球，aperture 处处 =1）
        #   p=1 时往「内」推（上下眼睑线互相越过 ≥2f，aperture 处处 =0，
        #                     含 arc=0 的左右两端也不会留 0.5 的缝）
        safety_up = f + curve_frac * h
        safety_dn = f + curve_frac * h * 0.5
        y_up_line = y_up - safety_up + arc + p * 2.0 * safety_up
        y_dn_line = y_dn + safety_dn - arc * 0.5 - p * 2.0 * safety_dn
        # 记录实际眼缝高度（几何判据，独立于 HSV 检测）
        slits.append(float(np.median(y_dn_line - y_up_line)))
        # 软边：feather 像素内线性过渡（物理存在的眼睑厚度）
        below_up = np.clip((ys - y_up_line[None, :]) / f + 0.5, 0.0, 1.0)
        above_dn = np.clip((y_dn_line[None, :] - ys) / f + 0.5, 0.0, 1.0)
        slit = np.minimum(below_up, above_dn)                       # (ph,ew) 眼缝
        # 只在眼睛 y 范围内用 slit；眼睛 y 范围之外（额头/脸颊）保持 1 不替换
        in_eye_y = ((ys >= float(ey0)) & (ys < float(ey1))).astype(np.float32)
        region = slit * in_eye_y + (1.0 - in_eye_y)                 # (ph,ew)
        ap[:, ex0:ex1] = np.minimum(ap[:, ex0:ex1], region)
    return ap[:, :, None], slits


def ghost_ratio(dst_patch: np.ndarray, src_patch: np.ndarray,
                blended: np.ndarray, region: np.ndarray | None = None) -> tuple[float, int]:
    """反解混合权重 m，统计"既不睁也不闭"的叠加态像素占比。

    m(x,y) = (blended - open) / (closed - open)
    纯线性混合 → m 处处等于该帧强度（单峰）→ 叠加态占比很高
    物理眼睑（wipe/aperture）→ m 双峰（≈0 或 ≈1）→ 只有眼睑过渡带那一小撮

    **region 参数很重要**：patch 四边有 9px 羽化（避免硬边接缝，是设计需要），
    那里 m 从 0 连续渐变到 1，会被误判成"叠加态"。不传 region 时 aperture 模式
    实测虚高到 16%，传了眼框 region 后降到 2%。**判据口径必须排除设计性过渡。**
    """
    O = dst_patch[:, :, :3]
    C = src_patch[:, :, :3]
    denom = np.abs(C - O).max(axis=2)
    valid = denom >= GHOST_MIN_DENOM
    if region is not None:
        valid &= region
    if not valid.any():
        return 0.0, 0
    num = (blended[:, :, :3] - O).max(axis=2)
    m = num / np.maximum(denom, 1e-6)
    mv = m[valid]
    return float(np.mean((mv > GHOST_LO) & (mv < GHOST_HI))), int(valid.sum())


def main() -> int:
    ap = argparse.ArgumentParser(description="程序化合成眨眼动作帧序列")
    ap.add_argument("--closed", required=True, help="闭眼版图（gpt-image-2 输出，通常 2048）")
    ap.add_argument("--master", required=True, help="母版 1024（闭眼图的对齐参考）")
    ap.add_argument("--anchor", required=True, help="呼吸循环锚点帧 f0000.png（588）")
    ap.add_argument("--out", required=True, help="输出目录")
    ap.add_argument("--fps", type=float, default=24.0)
    ap.add_argument("--eye-pad", type=int, default=EYE_PAD)
    ap.add_argument("--feather", type=int, default=FEATHER)
    ap.add_argument("--sequence", default=",".join(str(x) for x in CLOSE_SEQUENCE),
                    help="眼睑闭合程度序列 p（0=全睁 1=全闭），逗号分隔")
    ap.add_argument("--mode", default="blend", choices=["aperture", "wipe", "blend"],
                    help="blend=线性混合（默认。老王 2026-09-02 实拍定版：12帧504ms 下观感"
                         "柔和自然，虽然机械叠加态 70%%，但眼周仅占画布 2.7%%，肉眼分辨不出）；"
                         "aperture=上下眼皮向中间收拢（每帧独立物理画面、叠加态仅 7%%，"
                         "但连续播放时 1px 硬边眼皮每帧收拢 3~4px 显得生硬，老王未选）；"
                         "wipe=上眼睑从上往下扫（中间帧像半只眼，已淘汰）。"
                         "注意：机械叠加态指标只能排雷，不能选方案，最终必须看连续播放")
    ap.add_argument("--lid-curve", type=float, default=LID_CURVE_FRAC,
                    help="眼睑线弧度，占眼球高度的比例（0.15 = 中间比两边低眼球高度的15%%）")
    ap.add_argument("--lid-feather", type=int, default=LID_FEATHER, help="眼睑边缘过渡带宽度（像素）")
    ap.add_argument("--lid-margin", type=int, default=LID_MARGIN, help="扫描范围在眼球上下各外扩（像素）")
    ap.add_argument("--aperture-up-frac", type=float, default=APERTURE_UP_FRAC,
                    help="上眼睑贡献的闭合比例（0.70 = 上眼睑动 70%%、下眼睑动 30%%）")
    args = ap.parse_args()

    closed_path = Path(args.closed)
    master_path = Path(args.master)
    anchor_path = Path(args.anchor)
    out_dir = Path(args.out).resolve()
    for p in (closed_path, master_path, anchor_path):
        if not p.is_file():
            raise SystemExit(f"[缺输入] {p}")

    closed = imread_u(closed_path)
    master = imread_u(master_path)
    anchor = imread_u(anchor_path)
    print(f"[输入] 闭眼图 {closed.shape}  母版 {master.shape}  锚点 {anchor.shape}")

    # --- 1. 闭眼图缩到母版尺度（平台固定输出 2048） ---
    if closed.shape[0] != master.shape[0]:
        closed = cv2.resize(closed, (master.shape[1], master.shape[0]), interpolation=cv2.INTER_AREA)
        print(f"[缩放] 闭眼图 -> {closed.shape}")

    # --- 2. 闭眼图对齐到母版（同尺度，只平移） ---
    cm, mm = alpha_mask(closed), alpha_mask(master)
    cy0, cy1, cx0, cx1 = bbox_of(cm)
    my0, my1, mx0, mx1 = bbox_of(mm)
    dx, dy = mx0 - cx0, my0 - cy0
    dx, dy, cost = refine_shift(cm, mm, dx, dy, REFINE_RADIUS)
    inter = int(np.count_nonzero(np.roll(np.roll(cm, dy, axis=0), dx, axis=1) & mm))
    union = int(np.count_nonzero(np.roll(np.roll(cm, dy, axis=0), dx, axis=1) | mm))
    print(f"[对齐] 闭眼图→母版 dx={dx} dy={dy}  IoU={inter/union:.4f}")
    if inter / union < 0.90:
        raise SystemExit(f"[中止] 闭眼图与母版轮廓 IoU={inter/union:.3f} < 0.90，"
                         f"模型改了姿态/构图，这张图不能用（需重生成）")

    aligned = np.roll(np.roll(closed, dy, axis=0), dx, axis=1)

    # --- 3. 母版 → 锚点（588）的变换 ---
    scale = (anchor.shape[0] / master.shape[0])
    ay0, ay1, ax0, ax1 = bbox_of(alpha_mask(anchor))
    # 先按 bbox 对齐求初始 scale/offset
    s_h = (ay1 - ay0) / (my1 - my0)
    s_w = (ax1 - ax0) / (mx1 - mx0)
    s = (s_h + s_w) / 2.0
    print(f"[变换] 母版→锚点 bbox scale: 高 {s_h:.4f} 宽 {s_w:.4f} 取平均 {s:.4f}")

    # 闭眼图直接按同一变换映射到 588
    mapped = cv2.warpAffine(
        aligned, np.array([[s, 0, 0], [0, s, 0]], np.float32),
        (anchor.shape[1], anchor.shape[0]), flags=cv2.INTER_AREA,
        borderMode=cv2.BORDER_CONSTANT, borderValue=(0, 0, 0, 0),
    )
    # 平移对齐到锚点 bbox
    mp = alpha_mask(mapped)
    py0, _py1, px0, _px1 = bbox_of(mp)
    tdx, tdy = ax0 - px0, ay0 - py0
    tdx, tdy, cost = refine_shift(mp, alpha_mask(anchor), tdx, tdy, REFINE_RADIUS)
    mapped = np.roll(np.roll(mapped, tdy, axis=0), tdx, axis=1)
    inter = int(np.count_nonzero(alpha_mask(mapped) & alpha_mask(anchor)))
    union = int(np.count_nonzero(alpha_mask(mapped) | alpha_mask(anchor)))
    print(f"[对齐] 闭眼图→锚点 dx={tdx} dy={tdy}  IoU={inter/union:.4f}")
    if inter / union < 0.90:
        raise SystemExit(f"[中止] 闭眼图与锚点 IoU={inter/union:.3f} < 0.90，对齐不可靠")

    # --- 4. 定位眼睛并取 patch ---
    (ey0, ey1, ex0, ex1), eyes = detect_eyes(anchor)
    print(f"[眼睛] 左眼 y[{eyes[0][0]},{eyes[0][1]}] x[{eyes[0][2]},{eyes[0][3]}]  "
          f"右眼 y[{eyes[1][0]},{eyes[1][1]}] x[{eyes[1][2]},{eyes[1][3]}]")
    # 转成 patch 局部坐标（patch 比眼睛 bbox 外扩了 eye_pad）
    # x 方向额外外扩 LID_MARGIN：闭眼图的眼睑线比虹膜宽，只擦虹膜会漏出眼角
    x_pad_extra = max(args.lid_margin, 2)
    eyes_local = [(a - (ey0 - args.eye_pad), b - (ey0 - args.eye_pad),
                   c - (ex0 - args.eye_pad) - x_pad_extra,
                   d - (ex0 - args.eye_pad) + x_pad_extra)
                  for (a, b, c, d) in eyes]
    H, W = anchor.shape[:2]
    py0 = max(0, ey0 - args.eye_pad)
    py1 = min(H, ey1 + args.eye_pad)
    px0 = max(0, ex0 - args.eye_pad)
    px1 = min(W, ex1 + args.eye_pad)
    print(f"[眼睛] 合并 bbox y[{ey0},{ey1}] x[{ex0},{ex1}]  "
          f"替换矩形 y[{py0},{py1}] x[{px0},{px1}] ({px1-px0}x{py1-py0})")

    patch_src = mapped[py0:py1, px0:px1].astype(np.float32)
    patch_dst = anchor[py0:py1, px0:px1].astype(np.float32)
    feat = build_feather(patch_src.shape[0], patch_src.shape[1], args.feather)[:, :, None]

    # 只在锚点主体内部替换，避免把闭眼图的背景贴进来
    inside = (anchor[py0:py1, px0:px1, 3:4].astype(np.float32) / 255.0)
    # 闭眼图 patch 自身 alpha 也参与，避免半透明边缘污染
    src_alpha = patch_src[:, :, 3:4] / 255.0
    blend_mask = feat * inside * np.clip(src_alpha, 0.0, 1.0)

    # 残影判据的统计区域：只算眼框内，排除 patch 四边的 9px 羽化
    # （羽化是设计需要的软接缝，m 从 0 渐变到 1，会被误判成叠加态）
    patch_eye_region = np.zeros(patch_src.shape[:2], dtype=bool)
    for (a, b, c, d) in eyes_local:
        patch_eye_region[max(0, int(a)):int(b), max(0, int(c)):int(d)] = True

    seq = [float(x) for x in args.sequence.split(",") if x.strip()]
    print(f"[序列] {len(seq)} 帧  强度={seq}  总时长 {len(seq)/args.fps*1000:.0f}ms")

    frames_dir = out_dir / "frames"
    if frames_dir.exists():
        shutil.rmtree(frames_dir)
    frames_dir.mkdir(parents=True, exist_ok=True)

    out_frames = []
    ghost_reports = []
    slit_reports: list[list[float]] = []
    for i, k in enumerate(seq):
        if args.mode == "aperture":
            # aperture=1 露出眼球（用睁眼图）→ 替换程度 m = 0
            # aperture=0 被眼皮盖住（用闭眼图）→ 替换程度 m = 1
            ap, slits = build_aperture_mask(patch_src.shape[0], patch_src.shape[1], eyes_local,
                                            k, args.lid_feather, args.lid_curve,
                                            args.aperture_up_frac)
            m = blend_mask * (1.0 - ap)
            slit_reports.append(slits)
        elif args.mode == "wipe":
            lid = build_lid_mask(patch_src.shape[0], patch_src.shape[1], eyes_local,
                                 k, args.lid_feather, args.lid_curve, args.lid_margin)
            m = blend_mask * lid
        else:
            m = blend_mask * k
        blended = patch_dst * (1.0 - m) + patch_src * m
        frame = anchor.astype(np.float32).copy()
        frame[py0:py1, px0:px1] = blended
        frame_u = np.clip(frame, 0, 255).astype(np.uint8)
        name = f"f{i:04d}.png"
        imwrite_u(frames_dir / name, frame_u)
        out_frames.append(name)
        ghost_reports.append(ghost_ratio(patch_dst, patch_src, blended, patch_eye_region))

    # --- 5. 自检 ---
    print("\n[自检]")
    anchor_f = anchor.astype(np.float32)
    rect_area = (py1 - py0) * (px1 - px0)
    anchor_area = int(np.count_nonzero(anchor[:, :, 3] > 128))
    for i, name in enumerate(out_frames):
        f = imread_u(frames_dir / name).astype(np.float32)
        d = np.abs(f - anchor_f).mean(axis=2)
        changed = d > 8
        # 变化像素必须集中在替换矩形内
        outside = int(np.count_nonzero(changed))
        in_rect = int(np.count_nonzero(changed[py0:py1, px0:px1]))
        frac_in_rect = in_rect / outside if outside else 1.0
        area = int(np.count_nonzero(f[:, :, 3] > 128))
        gr, nvalid = ghost_reports[i]
        flag = "  <-- 残影" if gr > 0.25 else ""
        print(f"  {name} p={seq[i]:.2f}  变化像素={outside:6d} "
              f"(矩形内 {frac_in_rect*100:.1f}%)  轮廓面积 {area} (锚点 {anchor_area})  "
              f"叠加态 {gr*100:5.1f}%{flag}")
        if frac_in_rect < 0.95:
            print(f"     [警告] 有 {100-frac_in_rect*100:.1f}% 的变化落在眼周矩形之外")

    # 眨眼帧之间的轮廓面积必须完全一致（眼部替换不应改变轮廓）
    areas = [int(np.count_nonzero(imread_u(frames_dir / n)[:, :, 3] > 128)) for n in out_frames]
    spread = (max(areas) - min(areas)) / max(1, np.mean(areas)) * 100
    print(f"  眨眼帧轮廓面积波动 {spread:.3f}%  (应 ≈0)")
    print(f"  替换矩形占画布 {rect_area/(H*W)*100:.1f}%")

    # 残影总检：这是老王肉眼看到的"叠加态"，必须机械可查，不能只靠肉眼
    peak_ghost = max(g for g, _ in ghost_reports)
    print(f"  叠加态像素峰值 {peak_ghost*100:.1f}%  "
          f"(aperture/wipe 应 <10%，只剩眼睑过渡带；blend 会到 70%+ 那就是残影)")
    if args.mode in ("aperture", "wipe") and peak_ghost > 0.15:
        print(f"     [警告] {args.mode} 模式叠加态 {peak_ghost*100:.1f}% 偏高，"
              f"检查 --lid-feather 是否过大")

    # 眼缝收窄检查：这是"动作做没做对"的判据，不能只靠一致性检查
    # （一致性检查的盲区：一个纹丝不动的产物也能全 PASS）
    #
    # 【口径教训】第一版用"全图黄色像素数"当虹膜量，得出 p=1 时仍可见 65% 虹膜，
    # 误判成"眼睛没闭实"。实查：全图 1641 个黄色像素里真正的虹膜只有两块
    # （左 196px / 右 162px 共 358px），其余 238 个连通域全是 1~20px 的毛色高光噪声，
    # 散布在整只猫身上（甚至 y=567 的爪子）。**判据口径必须和遮罩口径对齐**，
    # 否则量的是噪声不是眼睛。
    def iris_in_eyes(img: np.ndarray) -> int:
        """只在 detect_eyes 给出的两只眼框内统计虹膜像素（与遮罩同口径）。"""
        a = img[:, :, 3]
        hsv = cv2.cvtColor(img[:, :, :3], cv2.COLOR_BGR2HSV)
        h = hsv[:, :, 0].astype(np.int16)
        s = hsv[:, :, 1].astype(np.int16)
        v = hsv[:, :, 2].astype(np.int16)
        m = (h >= 18) & (h <= 45) & (s >= 100) & (v >= 120) & (a > 128)
        total = 0
        for (ey0_, ey1_, ex0_, ex1_) in eyes:
            total += int(np.count_nonzero(m[ey0_:ey1_, ex0_:ex1_]))
        return total

    iris_anchor = iris_in_eyes(anchor)
    iris_seq = [iris_in_eyes(imread_u(frames_dir / n)) for n in out_frames]
    print(f"\n[眼缝收窄] 锚点(全睁) 眼框内虹膜像素 {iris_anchor}")
    for i, name in enumerate(out_frames):
        frac = iris_seq[i] / max(1, iris_anchor)
        expect = 1.0 - seq[i]
        geom = ""
        if slit_reports and i < len(slit_reports):
            geom = "  眼缝高 " + "/".join(f"{s:.1f}px" for s in slit_reports[i])
        print(f"  {name} p={seq[i]:.2f}  虹膜可见 {iris_seq[i]:5d} "
              f"(占全睁 {frac*100:5.1f}%，理论 {expect*100:5.1f}%){geom}")
    # 单调性：闭合段（p 递增到峰值前）虹膜必须单调递减
    peak_i = seq.index(max(seq))
    closing = iris_seq[:peak_i + 1]
    mono = all(closing[j] >= closing[j + 1] for j in range(len(closing) - 1))
    print(f"  闭合段单调收窄: {'是' if mono else '否'}  {closing}"
          + ("" if mono else "  [警告] 眼缝没有随 p 单调收窄"))
    if iris_seq[peak_i] > max(8, iris_anchor * 0.03):
        print(f"     [警告] p=1 帧眼框内仍有 {iris_seq[peak_i]} 个虹膜像素"
              f"（占全睁 {iris_seq[peak_i]/max(1,iris_anchor)*100:.1f}%），眼睛没闭实")
    # 几何判据（独立于 HSV）：眼缝高度必须随 p 单调递减，且 p=1 时 ≤0
    if slit_reports:
        peak_slits = slit_reports[peak_i]
        print(f"  几何眼缝高 p=1 时 {['%.1f' % s for s in peak_slits]} px  (应 ≤0)")
        if any(s > 0.0 for s in peak_slits):
            print("     [警告] p=1 时几何眼缝仍 >0，上下眼睑没有相遇")

    # 首尾帧必须能接回呼吸锚点帧，否则插入/退出时眼睛会突跳
    for idx, tag in ((0, "首帧"), (len(out_frames) - 1, "末帧")):
        f = imread_u(frames_dir / out_frames[idx]).astype(np.float32)
        d = float(np.abs(f - anchor_f).max())
        print(f"  {tag}与锚点帧最大像素差 {d:.1f}  "
              f"(p={seq[idx]:.2f}，应 ≈0，否则插入/退出有跳变)")
        if d > 8.0:
            print(f"     [警告] {tag} p={seq[idx]:.2f} ≠ 0，接回呼吸时眼睛会突跳")

    # --- 6. manifest (schemaVersion 7) ---
    manifest = {
        "schemaVersion": 7,
        "renderer": "frame-sequence-v1",
        "petId": out_dir.parent.parent.name,
        "variantId": "blink-composited",
        "displayName": "眨眼（合成）",
        "species": "cat",
        "baseImage": "frames/f0000.png",
        "defaultAction": "blink",
        "anchorPolicy": "fixed",
        "actions": [{
            "actionId": "blink",
            "loop": False,
            "frameDurationMs": round(1000 / args.fps),
            "frames": [f"frames/{n}" for n in out_frames],
        }],
        "semantics": {},
        "_provenance": {
            "generator": "scripts/poc_眨眼.py",
            "closedImage": str(closed_path),
            "master": str(master_path),
            "anchor": str(anchor_path),
            "closeSequence": seq,
            "eyeRect": [py0, py1, px0, px1],
            "eyePad": args.eye_pad,
            "feather": args.feather,
            "mode": args.mode,
            "lidCurveFrac": args.lid_curve,
            "lidFeather": args.lid_feather,
            "lidMargin": args.lid_margin,
            "peakGhostRatio": round(peak_ghost, 4),
            "note": ("wipe=物理眼睑扫描：上眼睑自上而下扫过眼球，每列像素非睁即闭，"
                     "只有眼睑边缘一条过渡带，杜绝睁闭叠加的残影。"
                     if args.mode == "wipe" else
                     "blend=旧线性混合：mid=open*(1-m)+closed*m，中间帧睁闭两态同时可见，有残影。")
                    + " 只替换眼周矩形，身体复用呼吸锚点帧。序列首尾 p=0，接回呼吸零跳变。",
        },
    }
    (out_dir / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")

    # 参考图：把替换矩形画出来，方便肉眼核对
    review = anchor.copy()
    cv2.rectangle(review, (px0, py0), (px1 - 1, py1 - 1), (0, 0, 255, 255), 1)
    imwrite_u(out_dir / "替换矩形.png", review)

    print(f"\n[输出] {len(out_frames)} 帧 -> {frames_dir}")
    print(f"[输出] manifest -> {out_dir / 'manifest.json'}")
    print(f"[输出] 替换矩形核对图 -> {out_dir / '替换矩形.png'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

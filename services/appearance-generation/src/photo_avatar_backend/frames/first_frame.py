# -*- coding: utf-8 -*-
"""透明母版 → 绿幕首帧（Seedance 图生视频的输入帧）。

从 `scripts/poc_绿幕首帧.py` **原样搬进服务**（算法零改动）：脚本现在只是这里的薄 CLI 包装。

关键约定：
  - 绿幕必须是纯 (0,255,0)，全程无渐变、无纹理、无阴影；
  - 主体 alpha 硬化到 255，避免母版残留的 252 造成整体偏绿；
  - 边缘保留原始抗锯齿过渡，因为这正是抠像要解决的难点。

产物：`母版-1024.png`（硬化并取景后的透明母版）、`绿幕首帧-1024.png`（无损）、
`绿幕首帧-1024.jpg`（默认，兼容性最好）、`首帧分析.json`。

⚠️ **取景余量是硬门槛**：`tailSwingFit` 里的左余量（`marginLeft`，名字有误导，它是
「母版左边界落在画布哪个比例」不是「左移多少」）与 `rightMarginAtFullSwing`
（尾巴甩到最远时的右余量）**都要 ≥ `MIN_FRAMING_MARGIN`**，否则摇尾会出画、
整支视频白花钱（长毛猫曾因此白花 4.67 算力）。不过关就调小 `scale` 重生成 —— 首帧免费。

⚠️ `analyze_master` 的 `thinStructureRatio` / `veryThinStructureRatio` 旧实现依赖 opencv，
装了就有值、没装就是 `null` —— 同脚本换机器产不同 JSON。现已改成 scipy（见该函数注释）。
"""
from __future__ import annotations

import json
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import numpy as np
from PIL import Image
from scipy import ndimage

from ._paths import rel_to

SIZE = 1024
GREEN = (0, 255, 0)
ALPHA_HARDEN = 240      # alpha >= 240 视为主体，硬化为 255
ALPHA_KILL = 16         # alpha <= 16 视为背景噪声，清零

# 摇尾时尾尖摆幅 / 猫身宽度。由 1:1 那支实测：右边界在 795~959 间摆动，
# 摆幅 164px，单帧猫宽 699px -> 0.235。换宠物或换动作幅度要重新测。
TAIL_SWING_RATIO = 0.235

# 两个余量的下限。任一低于它就不许进视频（见模块开头）。
# 注意：`scripts/一键出宠.py` 里还有一个 `MARGIN_MIN = 0.05`，
# 等第 4 片 `frames/pipeline.py` 串链时统一到这一处。
MIN_FRAMING_MARGIN = 0.05


class FirstFrameError(ValueError):
    """首帧合成的失败态。必须是 `Exception` 子类 —— 见 `frames/__init__.py` 的约定。"""


def _erode_ratio(mask: np.ndarray, iterations: int) -> float:
    """腐蚀 `iterations` 次后消失的像素占前景的比例。

    `border_value=True` 是刻意的：把画布外当作前景，边界上的前景就不会被吃掉 ——
    这与 cv2 的 `morphologyDefaultBorderValue` 语义一致，数值可与旧实现逐位对上。
    """
    eroded = ndimage.binary_erosion(
        mask, structure=np.ones((3, 3), dtype=bool),
        iterations=iterations, border_value=True,
    )
    area = float(mask.sum())
    return round(float((mask.sum() - eroded.sum()) / area), 4) if area else 0.0


def analyze_master(mask: np.ndarray) -> dict:
    """统计母版前景占比、包围盒，以及细结构（胡须、毛尖）占比。

    `thinStructureRatio` / `veryThinStructureRatio` 用形态学腐蚀估：腐蚀一次（/两次）
    就消失的部分即宽度 <=2px（/<=4px）的细结构，对应胡须与毛尖 —— 它们最容易在
    抠像时被切掉，所以要在首帧阶段就记下来。

    ⚠️ **旧实现是 `try: import cv2`**：装了 opencv 就出数字、没装就出 `null`。
    同一个脚本换台机器跑就产不同的 JSON，对要进服务的模块是不可接受的隐藏依赖
    （2026-09-13 实测：Python312 有 cv2 → 出数字；后端解释器没有 → 出 null）。
    现已固定成 scipy（本模块的既有依赖），数值与 cv2 路径逐位一致。
    """
    stats = {
        "foregroundRatio": round(float(mask.mean()), 4),
    }
    ys, xs = np.nonzero(mask)
    if ys.size:
        stats["bbox"] = {
            "x": int(xs.min()), "y": int(ys.min()),
            "w": int(xs.max() - xs.min() + 1), "h": int(ys.max() - ys.min() + 1),
        }
        stats["bboxFillRatio"] = round(float(mask[ys.min():ys.max() + 1, xs.min():xs.max() + 1].mean()), 4)
    else:
        stats["bbox"] = None
        stats["bboxFillRatio"] = 0.0

    stats["thinStructureRatio"] = _erode_ratio(mask, 1)
    stats["veryThinStructureRatio"] = _erode_ratio(mask, 2)
    return stats


def prepare_master(src: Path, *, size: int = SIZE) -> tuple[Image.Image, dict]:
    """读取母版 -> 硬化 alpha -> 缩放到 `size`。返回 (图, 硬化前的原始统计)。"""
    im = Image.open(src).convert("RGBA")
    arr = np.array(im).astype(np.float32)
    a = arr[:, :, 3]

    raw = {
        "sourceSize": list(im.size),
        "alpha0Ratio": round(float((a == 0).mean()), 4),
        "alpha255Ratio": round(float((a == 255).mean()), 4),
        "coreRatio": round(float((a >= ALPHA_HARDEN).mean()), 4),
        "edgeBandRatio": round(float(((a > ALPHA_KILL) & (a < ALPHA_HARDEN)).mean()), 4),
    }

    a = np.where(a >= ALPHA_HARDEN, 255.0, a)
    a = np.where(a <= ALPHA_KILL, 0.0, a)
    arr[:, :, 3] = a

    hardened = Image.fromarray(arr.astype(np.uint8), mode="RGBA")
    hardened = hardened.resize((size, size), Image.LANCZOS)
    return hardened, raw


def reposition_for_tail_swing(
    master: Image.Image,
    scale: float,
    margin_left: float,
    *,
    size: int = SIZE,
) -> tuple[Image.Image, dict]:
    """把母版整体缩小并左移，给尾巴甩动留出空间。

    为什么会需要这一步（1:1 画幅实测踩的坑）：
    母版里猫的包围盒是 x[0.225, 0.949] —— 尾巴尖离右边界只剩 5%。
    而摇尾时尾尖摆幅可达**猫宽的 23.5%**，一甩就出画。
    实测 1:1 / 960×960 那支，121 帧里有 23 帧尾巴被切，右边界截面宽 64-78px，
    等于整条尾巴被齐刷刷切断。

    所以要么换宽画幅（但 16:9 只有 496px，分辨率砍半），
    要么在这里把猫缩小左移、腾出右侧空间 —— 能保住 960 分辨率。
    """
    alpha = np.array(master)[:, :, 3]
    ys, xs = np.nonzero(alpha >= 128)
    if not ys.size:
        raise FirstFrameError("母版里没有 alpha>=128 的主体像素，无法取景")
    x0, x1 = xs.min() / size, xs.max() / size
    y0, y1 = ys.min() / size, ys.max() / size
    cat_w, cat_h = x1 - x0, y1 - y0

    side = int(round(size * scale))
    scaled = master.resize((side, side), Image.LANCZOS)

    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ox = int(round(margin_left * size - x0 * side))
    oy = int(round((size - cat_h * side) / 2 - y0 * side))
    sx0 = max(0, -ox)
    sy0 = max(0, -oy)
    canvas.paste(scaled.crop((sx0, sy0, side, side)), (max(0, ox), max(0, oy)))

    # 复核：算一下尾巴甩到最远时还剩多少余量
    swing = cat_w * TAIL_SWING_RATIO * scale
    right_edge = margin_left + cat_w * scale + swing
    vert_margin = (1.0 - cat_h * scale) / 2
    info = {
        "scale": scale,
        "marginLeft": margin_left,
        "catBoxNormalized": [round(x0, 4), round(y0, 4), round(x1, 4), round(y1, 4)],
        "catWidthAfterScale": round(cat_w * scale, 4),
        "catHeightAfterScale": round(cat_h * scale, 4),
        "tailSwingEstimate": round(swing, 4),
        "rightMarginAtFullSwing": round(1.0 - right_edge, 4),
        "verticalMargin": round(vert_margin, 4),
        "tailSwingRatioUsed": TAIL_SWING_RATIO,
    }
    return canvas, info


def composite_green(master: Image.Image, *, green: tuple[int, int, int] = GREEN) -> Image.Image:
    """把透明母版合成到纯绿幕上（RGB 输出）。"""
    bg = Image.new("RGB", master.size, green)
    bg = bg.convert("RGBA")
    return Image.alpha_composite(bg, master).convert("RGB")


@dataclass(frozen=True)
class FirstFrameResult:
    out_dir: Path
    master_png: Path
    frame_png: Path
    frame_jpg: Path
    analysis_path: Path
    report: dict
    selected: str
    size: int
    background_green_exact_ratio: float

    @property
    def tail_swing_fit(self) -> dict | None:
        """`scale == 1.0` 时不做取景重排，这里就是 None。"""
        return self.report.get("tailSwingFit")

    @property
    def left_margin(self) -> float | None:
        fit = self.tail_swing_fit
        return None if fit is None else float(fit["marginLeft"])

    @property
    def right_margin_at_full_swing(self) -> float | None:
        fit = self.tail_swing_fit
        return None if fit is None else float(fit["rightMarginAtFullSwing"])

    @property
    def framing_ok(self) -> bool:
        """两个余量都 >= `MIN_FRAMING_MARGIN` 才算取景合格。

        `scale == 1.0`（没做重排）时无余量可判 → 视为不合格，调用方必须给 scale < 1.0。
        """
        left, right = self.left_margin, self.right_margin_at_full_swing
        if left is None or right is None:
            return False
        return left >= MIN_FRAMING_MARGIN and right >= MIN_FRAMING_MARGIN


def compose_green_first_frame(
    masters: Sequence[tuple[str, Path]],
    out_dir: Path,
    *,
    scale: float = 0.9,
    margin_left: float = 0.05,
    size: int = SIZE,
    green: tuple[int, int, int] = GREEN,
    path_base: Path | None = None,
    log: Callable[[str], None] = print,
) -> FirstFrameResult:
    """把透明母版合成到纯绿幕上，产出一支 Seedance 图生视频的首帧。

    `masters` 是 `(风格名, 母版 PNG)` 的候选表：会逐个做 `analyze_master`，
    选 `edgeBandRatio`（半透明边缘带占比）最小的那个当首帧 —— 边缘越干净，
    抠像越省事。产品路径只会传一个候选；多候选是 POC 的比对便利。

    `scale < 1.0` 时走 `reposition_for_tail_swing` 腾出甩尾空间；
    `scale == 1.0` 表示不动取景（此时 `framing_ok` 恒为 False，会被闸口拦下）。
    """
    out_dir = Path(out_dir)
    if not masters:
        raise FirstFrameError("没有候选母版")
    if not 0 < scale <= 1.0:
        raise FirstFrameError(f"scale 必须在 (0, 1] 内: {scale}")
    if not 0.0 <= margin_left <= 1.0:
        raise FirstFrameError(f"margin_left 必须在 [0, 1] 内: {margin_left}")
    if size <= 0:
        raise FirstFrameError(f"size 必须为正: {size}")

    out_dir.mkdir(parents=True, exist_ok=True)
    report = {"size": size, "green": list(green), "candidates": []}

    usable: list[tuple[str, Path]] = []
    for name, path in masters:
        path = Path(path)
        if not path.is_file():
            log(f"[SKIP] 缺少母版: {path}")
            continue
        master, raw = prepare_master(path, size=size)
        stats = analyze_master(np.array(master)[:, :, 3] >= 128)
        report["candidates"].append(
            {"style": name, "source": rel_to(path, path_base), **raw, **stats}
        )
        usable.append((name, path))
        log(f"[分析] {name}: 主体占比={stats['foregroundRatio']:.3f} "
            f"细结构={stats['thinStructureRatio']} 边缘带={raw['edgeBandRatio']:.4f}")

    if not usable:
        raise FirstFrameError(
            "没有可用母版：" + ", ".join(str(Path(p)) for _, p in masters)
        )

    # 选边缘带最窄的作为首帧
    best = sorted(report["candidates"], key=lambda e: e["edgeBandRatio"])[0]
    report["selected"] = best["style"]
    log(f"\n[选定] 首帧母版 = {best['style']}")

    src_path = next(p for n, p in usable if n == best["style"])
    master, _ = prepare_master(src_path, size=size)

    # 缩小并左移，给尾巴甩动腾出右侧空间（1:1 画幅下不这么做会切掉整条尾巴）
    if scale < 1.0:
        master, fit = reposition_for_tail_swing(master, scale, margin_left, size=size)
        report["tailSwingFit"] = fit
        log(f"[取景] 猫缩放 {scale:.2f}、左边距 {margin_left:.2f}")
        log(f"       尾巴甩到最远时右侧余量 {fit['rightMarginAtFullSwing'] * 100:.1f}%"
            f"  上下余量 {fit['verticalMargin'] * 100:.1f}%")
        if fit["rightMarginAtFullSwing"] < MIN_FRAMING_MARGIN:
            log(f"[警告] 右侧余量不足 {MIN_FRAMING_MARGIN * 100:.0f}%，尾巴仍可能出画，请调小 scale")

    master_path = out_dir / "母版-1024.png"
    master.save(master_path)

    frame = composite_green(master, green=green)
    # JPG：兼容性最好，作为默认首帧（已关闭色彩二次采样，质量 95）
    frame_path = out_dir / "绿幕首帧-1024.jpg"
    frame.save(frame_path, quality=95, subsampling=0)
    # PNG：无损备选，若 Seedance 支持 PNG 上传应优先使用
    png_path = out_dir / "绿幕首帧-1024.png"
    frame.save(png_path)

    # 首帧自检：绿幕纯度
    farr = np.array(frame).astype(np.int16)
    fg = np.array(master)[:, :, 3] >= 128
    bg_pixels = farr[~fg]
    green_exact = int(np.all(bg_pixels == np.array(green, dtype=np.int16), axis=1).sum())
    green_ratio = round(green_exact / max(1, bg_pixels.shape[0]), 6)
    report["firstFrame"] = {
        "path": rel_to(frame_path, path_base),
        "losslessPath": rel_to(png_path, path_base),
        "masterPath": rel_to(master_path, path_base),
        "size": list(frame.size),
        "backgroundGreenExactRatio": green_ratio,
        "backgroundPixelCount": int(bg_pixels.shape[0]),
    }
    log(f"[输出] {frame_path}")
    log(f"[自检] 背景纯绿像素占比 = {green_ratio:.6f}")

    analysis_path = out_dir / "首帧分析.json"
    analysis_path.write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    return FirstFrameResult(
        out_dir=out_dir,
        master_png=master_path,
        frame_png=png_path,
        frame_jpg=frame_path,
        analysis_path=analysis_path,
        report=report,
        selected=best["style"],
        size=size,
        background_green_exact_ratio=green_ratio,
    )

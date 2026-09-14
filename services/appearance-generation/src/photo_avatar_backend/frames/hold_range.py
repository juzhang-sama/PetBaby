# -*- coding: utf-8 -*-
"""交互动作（`grab-release`）的「悬空保持段」体检。

## 这个模块**不给结论**，只给事实

`manifest.actions[i].holdRange` 是「悬空保持」的帧下标闭区间：播放器进入该区间后
不再前进，而是在区间内循环，直到调用方以「释放」语义再触发同一动作。
它**必须由人定**——理由见下。

### 为什么不做「自动标定」

2026-09-14 在 05 的真实资产上把三条线索都试了，互相矛盾：

1. **`scripts/poc_拎起复查.py` 的规则**（「宽度 ≤ 最窄×1.15 的连续段」）跑在 **120 帧
   原视频**上，还会退化出 `[0,120]`（全段）这种无效值 —— 它是**速诊**，不是出厂值。
2. **`scripts/poc_提起放下验收.py` 的设计比例**（`0.16..0.46`）× 81 帧 = `[12,37]`，
   而 05 出厂写的是 **`[30,59]`** —— 对不上。
3. 最直觉的判据「区间首尾帧相似」**不成立**：`IoU(f0030, f0059) = 0.41`。
   `IoU(f0000, fi)` 在 f34 触底（0.369，悬空最高点），到 f59 已回到 0.888（接近端坐）
   —— 说明 `holdRange` 是**窗口**语义（在窗口内来回循环），不是「循环点」语义。

**结论**：历史上是人眼定稿的，没有可复现的算法。硬做自动标定 = 装一个
「参数看着合法、体验随机」的拎起。所以：

- **值从动作配置来**（`actions/<id>.json` 里可选的 `holdRange`，人填）；
- **本模块只体检**：把候选段、循环 IoU、宽度波动算出来写进日志/审计，
  给真跑时人眼定稿用。**一个字都不写进 manifest。**

### 「不写」是安全的降级

少了 `holdRange` 只是拎起来**没有悬空保持**（播完就落回），
而不是一个错的值 —— 与第 4 片 `ExtraAction.hold_range=None` 的口径一致。
"""
from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image

__all__ = [
    "LOOP_IOU_REFERENCE",
    "HoldRangeCandidate",
    "HoldRangeReport",
    "inspect_hold_range",
]

# 静默（alpha 全透明）的帧不算数。
ALPHA_THRESHOLD = 32

# 候选段的宽度容差：≤ 全局最窄 × 这个系数都算「窄」。
# 沿用 `poc_拎起复查.py` 的 1.15 —— 换个系数就等于换了个候选，所以写死在这里并说明出处。
NARROW_TOLERANCE = 1.15

# 循环 IoU 的参考线（`poc_提起放下验收.py` 用的判据）。**只是参考**：
# 05 的出厂值只有 0.41，所以这条线过不了的候选未必是错的 —— 它只说明
# 「窗口首尾长得不像」。
LOOP_IOU_REFERENCE = 0.9


@dataclass(frozen=True)
class HoldRangeCandidate:
    """一个候选窗口及其体检数字。"""

    span: tuple[int, int]
    frame_count: int
    width_span_px: int
    loop_iou: float

    @property
    def looks_loopable(self) -> bool:
        return self.loop_iou >= LOOP_IOU_REFERENCE


@dataclass(frozen=True)
class HoldRangeReport:
    """体检报告。**不是结论** —— 值该写什么由人定。"""

    frame_count: int
    widths: tuple[int, ...]
    narrowest_width: int
    candidate: HoldRangeCandidate | None
    configured: tuple[int, int] | None
    # 配置窗口的循环 IoU。**只报数**：05 的出厂值只有 0.41，
    # 用它当闸门会把正确值拒掉（见 `test_the_real_builtin_grab_release_*`）。
    configured_loop_iou: float | None = None

    def log_lines(self) -> list[str]:
        lines = [
            f"[悬空体检] {self.frame_count} 帧；宽度 最窄={self.narrowest_width} "
            f"最宽={max(self.widths) if self.widths else 0}"
        ]
        if self.candidate is None:
            lines.append("[悬空体检] 没有找到候选段（宽度没有明显的「窄」段）")
        else:
            lo, hi = self.candidate.span
            lines.append(
                f"[悬空体检] 候选窗口 f{lo:04d}~f{hi:04d}（{self.candidate.frame_count} 帧，"
                f"宽度波动 {self.candidate.width_span_px}px，循环 IoU "
                f"{self.candidate.loop_iou:.3f}，参考线 {LOOP_IOU_REFERENCE}）"
            )
        if self.configured is None:
            lines.append(
                "[悬空体检] 动作配置里没有 holdRange → 包里不写这个键"
                "（拎起来没有「悬空保持」，播完就落回；这是安全降级，不是错值）"
            )
        else:
            lo, hi = self.configured
            lines.append(f"[悬空体检] 配置的窗口 f{lo:04d}~f{hi:04d}（人定的，服务只读）")
        return lines


def _frames_dir_alpha_masks(frames_dir: Path) -> list[np.ndarray]:
    """读抠像后的 RGBA 帧，取 alpha ≥ 阈值的二值掩码。

    用 alpha 而不是重新算绿幕：这一步的输入**已经是抠好的帧**（见 `matting`），
    再算一遍绿幕等于引入第二个「什么算前景」的口径。
    """
    paths = sorted(frames_dir.glob("f*.png")) or sorted(frames_dir.glob("f*.webp"))
    masks: list[np.ndarray] = []
    for path in paths:
        with Image.open(path) as image:
            alpha = np.array(image.convert("RGBA"))[:, :, 3]
        masks.append(alpha >= ALPHA_THRESHOLD)
    return masks


def _widths(masks: list[np.ndarray]) -> list[int]:
    widths: list[int] = []
    for mask in masks:
        cols = np.nonzero(mask.any(axis=0))[0]
        widths.append(int(cols.max() - cols.min() + 1) if cols.size else 0)
    return widths


def _narrowest_run(widths: list[int]) -> tuple[int, int] | None:
    """最长的「窄」连续段（宽度 ≤ 最窄 × 容差）。"""
    if not widths:
        return None
    narrowest = min(width for width in widths if width > 0) if any(widths) else 0
    if narrowest <= 0:
        return None
    narrow = [i for i, width in enumerate(widths) if width <= narrowest * NARROW_TOLERANCE]
    if not narrow:
        return None
    runs: list[list[int]] = [[narrow[0]]]
    for i in narrow[1:]:
        if i == runs[-1][-1] + 1:
            runs[-1].append(i)
        else:
            runs.append([i])
    best = max(runs, key=len)
    return best[0], best[-1]


def _iou(a: np.ndarray, b: np.ndarray) -> float:
    union = int(np.logical_or(a, b).sum())
    return float(np.logical_and(a, b).sum()) / float(union) if union else 0.0


def inspect_hold_range(
    frames_dir: Path,
    *,
    configured: tuple[int, int] | None = None,
    log: Callable[[str], None] = print,
) -> HoldRangeReport:
    """体检一支交互动作的帧序列，报告「悬空保持段」的候选与数字。

    `configured` 是**动作配置里人定的那个值**（有就一并体检，看它到底好不好）。
    本函数**不写任何东西** —— 报告由调用方决定怎么用。
    """
    frames_dir = Path(frames_dir)
    if not frames_dir.is_dir():
        raise ValueError(f"frames directory does not exist: {frames_dir}")
    masks = _frames_dir_alpha_masks(frames_dir)
    if not masks:
        raise ValueError(f"no frames in {frames_dir}")
    widths = _widths(masks)
    span = _narrowest_run(widths)

    candidate = None
    if span is not None:
        lo, hi = span
        window = widths[lo : hi + 1]
        candidate = HoldRangeCandidate(
            span=span,
            frame_count=hi - lo + 1,
            width_span_px=max(window) - min(window),
            loop_iou=_iou(masks[lo], masks[hi]),
        )

    configured_span = None
    configured_iou = None
    if configured is not None:
        lo, hi = configured
        if 0 <= lo <= hi < len(masks):
            configured_span = configured
            configured_iou = _iou(masks[lo], masks[hi])
        else:
            log(f"[悬空体检] ⚠️ 配置的 holdRange {configured} 越界（共 {len(masks)} 帧）→ 忽略")

    report = HoldRangeReport(
        frame_count=len(masks),
        widths=tuple(widths),
        narrowest_width=min((w for w in widths if w > 0), default=0),
        candidate=candidate,
        configured=configured_span,
        configured_loop_iou=configured_iou,
    )
    for line in report.log_lines():
        log(line)
    if configured_iou is not None:
        log(
            f"[悬空体检] 配置窗口的循环 IoU = {configured_iou:.3f}"
            f"（参考线 {LOOP_IOU_REFERENCE}，低于它只说明首尾长得不像，**不是判失败**）"
        )
    return report

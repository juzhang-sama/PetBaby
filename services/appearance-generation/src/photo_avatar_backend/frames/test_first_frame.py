# -*- coding: utf-8 -*-
"""首帧合成测试。

只验「几何 / alpha 硬化 / 自检」这些纯逻辑，不验 Seedance 怎么消费这支首帧。
用 64×64 的小母版跑端到端 —— `size` 是参数，不必为了测试生成 1024²。
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest
from PIL import Image

from photo_avatar_backend.frames.first_frame import (
    ALPHA_HARDEN,
    ALPHA_KILL,
    GREEN,
    MIN_FRAMING_MARGIN,
    FirstFrameError,
    analyze_master,
    compose_green_first_frame,
    composite_green,
    prepare_master,
    reposition_for_tail_swing,
)

SIZE = 64


def write_master(path: Path, *, size: int = SIZE, box=(16, 8, 48, 56),
                 edge_alpha: int | None = None) -> Path:
    """造一张透明母版：中间一块 alpha=255 的主体，其余透明。"""
    arr = np.zeros((size, size, 4), np.uint8)
    arr[:, :, :3] = (200, 120, 60)
    x0, y0, x1, y1 = box
    arr[y0:y1, x0:x1, 3] = 255
    if edge_alpha is not None:
        # 外扩一圈半透明，模拟抗锯齿边缘
        arr[max(0, y0 - 1):y1 + 1, max(0, x0 - 1):x1 + 1, 3] = edge_alpha
        arr[y0:y1, x0:x1, 3] = 255
    path.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray(arr, mode="RGBA").save(path)
    return path


# ------------------------------------------------------------------ prepare_master

def test_prepare_master_hardens_and_clears_alpha(tmp_path: Path):
    size = SIZE
    arr = np.zeros((size, size, 4), np.uint8)
    arr[:, :, :3] = 100
    arr[10:20, 10:20, 3] = ALPHA_HARDEN          # 240 -> 255
    arr[30:40, 30:40, 3] = ALPHA_HARDEN - 1      # 239 -> 原样保留
    arr[50:56, 10:16, 3] = ALPHA_KILL            # 16 -> 0
    arr[50:56, 20:26, 3] = ALPHA_KILL + 1        # 17 -> 原样保留
    src = tmp_path / "m.png"
    Image.fromarray(arr, mode="RGBA").save(src)

    master, raw = prepare_master(src, size=size)
    alpha = np.array(master)[:, :, 3]

    assert (alpha[10:20, 10:20] == 255).all(), "alpha >= 240 必须硬化为 255"
    assert (alpha[30:40, 30:40] == ALPHA_HARDEN - 1).all(), "240 以下不该动"
    assert (alpha[50:56, 10:16] == 0).all(), "alpha <= 16 必须清零"
    assert (alpha[50:56, 20:26] == ALPHA_KILL + 1).all(), "16 以上不该动"

    assert master.size == (size, size)
    assert raw["sourceSize"] == [size, size]
    # 硬化发生在统计之后：alpha255Ratio 数的是「硬化前的原生 255」
    assert raw["alpha255Ratio"] == 0.0
    assert set(raw) == {"sourceSize", "alpha0Ratio", "alpha255Ratio", "coreRatio", "edgeBandRatio"}


# ------------------------------------------------------------------ analyze_master

def test_analyze_master_reports_bbox_and_ratios():
    mask = np.zeros((SIZE, SIZE), bool)
    mask[8:56, 16:48] = True

    stats = analyze_master(mask)

    assert stats["bbox"] == {"x": 16, "y": 8, "w": 32, "h": 48}
    assert stats["bboxFillRatio"] == 1.0, "实心矩形的包围盒内应全为前景"
    assert stats["foregroundRatio"] == round(32 * 48 / (SIZE * SIZE), 4)


def test_analyze_master_reports_thin_structure_without_opencv():
    """细结构占比必须**与环境无关**。

    旧实现是 `try: import cv2` —— 装了 opencv 出数字、没装出 `null`，
    同一个脚本换台机器跑就产不同的 JSON（2026-09-13 实测踩到）。
    现在固定走 scipy，这里断言它恒为数字、且腐蚀两次比一次丢得多。
    """
    mask = np.zeros((SIZE, SIZE), bool)
    mask[8:56, 16:48] = True

    stats = analyze_master(mask)

    assert isinstance(stats["thinStructureRatio"], float)
    assert isinstance(stats["veryThinStructureRatio"], float)
    assert 0.0 < stats["thinStructureRatio"] < 1.0
    assert stats["veryThinStructureRatio"] > stats["thinStructureRatio"], "多腐蚀一次应丢更多"


def test_analyze_master_handles_empty_mask():
    stats = analyze_master(np.zeros((SIZE, SIZE), bool))

    assert stats["bbox"] is None
    assert stats["bboxFillRatio"] == 0.0
    assert stats["foregroundRatio"] == 0.0
    assert stats["thinStructureRatio"] == 0.0


# ------------------------------------------------------------------ composite_green

def test_composite_paints_exact_green_behind_transparent_pixels(tmp_path: Path):
    src = write_master(tmp_path / "m.png")
    master, _ = prepare_master(src, size=SIZE)

    frame = composite_green(master)
    arr = np.array(frame)

    assert frame.mode == "RGB"
    alpha = np.array(master)[:, :, 3]
    assert (arr[alpha == 0] == np.array(GREEN, dtype=np.uint8)).all(), "背景必须是纯绿"
    assert (arr[alpha == 255] == np.array([200, 120, 60], dtype=np.uint8)).all(), "主体颜色不该被动"


# ------------------------------------------------------------------ 取景重排

def _rgba_with_box(size: int, box) -> np.ndarray:
    arr = np.zeros((size, size, 4), np.uint8)
    arr[:, :, :3] = (200, 120, 60)
    x0, y0, x1, y1 = box
    arr[y0:y1, x0:x1, 3] = 255
    return arr


def test_reposition_scales_and_places_left_margin():
    master = Image.fromarray(_rgba_with_box(SIZE, (16, 8, 48, 56)), mode="RGBA")

    canvas, fit = reposition_for_tail_swing(master, 0.5, 0.10, size=SIZE)
    alpha = np.array(canvas)[:, :, 3]
    ys, xs = np.nonzero(alpha >= 128)

    # 原主体 x ∈ [16,48) 宽 32；缩 0.5 后宽 16，左边界落在 0.10*64 = 6.4 -> 6
    assert canvas.size == (SIZE, SIZE)
    assert abs(int(xs.min()) - 6) <= 1
    assert abs((xs.max() - xs.min() + 1) - 16) <= 1
    assert set(fit) == {
        "scale", "marginLeft", "catBoxNormalized", "catWidthAfterScale",
        "catHeightAfterScale", "tailSwingEstimate", "rightMarginAtFullSwing",
        "verticalMargin", "tailSwingRatioUsed",
    }
    # 尾巴甩到最远时的右余量 = 1 - (左边距 + 猫宽 + 摆幅)
    expected_right = 1.0 - (0.10 + fit["catWidthAfterScale"] + fit["tailSwingEstimate"])
    assert fit["rightMarginAtFullSwing"] == round(expected_right, 4)
    assert fit["tailSwingEstimate"] == round(fit["catWidthAfterScale"] * fit["tailSwingRatioUsed"], 4)


def test_reposition_rejects_master_without_subject():
    empty = Image.fromarray(np.zeros((SIZE, SIZE, 4), np.uint8), mode="RGBA")
    with pytest.raises(FirstFrameError, match="alpha>=128"):
        reposition_for_tail_swing(empty, 0.9, 0.05, size=SIZE)


# ------------------------------------------------------------------ 端到端

def test_compose_writes_all_artifacts_and_report_shape(tmp_path: Path):
    master = write_master(tmp_path / "母版.png")
    out = tmp_path / "out"

    result = compose_green_first_frame([("候选", master)], out, scale=0.9, margin_left=0.06, size=SIZE)

    assert result.master_png.is_file()
    assert result.frame_png.is_file()
    assert result.frame_jpg.is_file()
    assert result.analysis_path.is_file()

    report = json.loads(result.analysis_path.read_text(encoding="utf-8"))
    assert list(report) == ["size", "green", "candidates", "selected", "tailSwingFit", "firstFrame"]
    assert list(report["candidates"][0])[:2] == ["style", "source"]
    assert list(report["firstFrame"]) == [
        "path", "losslessPath", "masterPath", "size",
        "backgroundGreenExactRatio", "backgroundPixelCount",
    ]
    assert report["size"] == SIZE
    assert report["green"] == list(GREEN)
    assert report["selected"] == "候选"
    assert report["firstFrame"]["size"] == [SIZE, SIZE]
    # 背景全是纯绿：这张母版的主体很小，背景占比应接近 1
    assert report["firstFrame"]["backgroundGreenExactRatio"] > 0.9
    assert result.background_green_exact_ratio == report["firstFrame"]["backgroundGreenExactRatio"]


def test_compose_picks_the_cleanest_master(tmp_path: Path):
    """选 `edgeBandRatio` 最小的候选 —— 半透明边缘带越窄，抠像越省事。"""
    blurry = write_master(tmp_path / "blurry.png", edge_alpha=100)   # 半透明带很宽
    clean = write_master(tmp_path / "clean.png")                     # 只有硬边

    result = compose_green_first_frame(
        [("blurry", blurry), ("clean", clean)], tmp_path / "out", size=SIZE
    )

    assert result.selected == "clean"


def test_framing_gate_flags_margins(tmp_path: Path):
    master = write_master(tmp_path / "母版.png")

    ok = compose_green_first_frame([("m", master)], tmp_path / "a", scale=0.9, margin_left=0.06, size=SIZE)
    assert ok.left_margin == 0.06
    assert ok.framing_ok is True, "0.06 与 12.8% 右余量都该过关"

    # 放大到 1.0 左右 + 左边距给足，右侧必然吃紧
    tight = compose_green_first_frame(
        [("m", master)], tmp_path / "b", scale=1.0, margin_left=0.5, size=SIZE
    )
    assert tight.tail_swing_fit is None
    assert tight.framing_ok is False, "没做重排（scale=1.0）就没有余量可判，必须视为不合格"


def test_compose_skips_missing_master_but_uses_the_rest(tmp_path: Path):
    good = write_master(tmp_path / "good.png")

    result = compose_green_first_frame(
        [("missing", tmp_path / "nope.png"), ("good", good)], tmp_path / "out", size=SIZE
    )

    assert result.selected == "good"
    assert [c["style"] for c in result.report["candidates"]] == ["good"]


@pytest.mark.parametrize(
    ("masters", "kwargs", "match"),
    [
        ([], {}, "没有候选母版"),
        ([("m", Path("nowhere.png"))], {}, "没有可用母版"),
        ([("m", Path("nowhere.png"))], {"scale": 0.0}, "scale"),
        ([("m", Path("nowhere.png"))], {"scale": 1.5}, "scale"),
        ([("m", Path("nowhere.png"))], {"scale": 0.9, "margin_left": -0.1}, "margin_left"),
        ([("m", Path("nowhere.png"))], {"scale": 0.9, "size": 0}, "size"),
    ],
)
def test_rejects_invalid_arguments(tmp_path: Path, masters, kwargs, match):
    with pytest.raises(FirstFrameError, match=match):
        compose_green_first_frame(masters, tmp_path / "out", **kwargs)


def test_first_frame_error_is_a_normal_exception():
    """`SystemExit` 不是 `Exception`，job_store 的 `except Exception` 抓不到 —— 会静默逃逸。"""
    assert not issubclass(FirstFrameError, SystemExit)
    assert issubclass(FirstFrameError, Exception)
    with pytest.raises(Exception):
        compose_green_first_frame([], Path("unused"))


def test_min_framing_margin_matches_the_documented_gate():
    assert MIN_FRAMING_MARGIN == 0.05

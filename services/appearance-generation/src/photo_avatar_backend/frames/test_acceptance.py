# -*- coding: utf-8 -*-
"""四项验收判据的测试。

用**人工合成的帧**验判据的逻辑（哪些信号算缺陷、哪些算正常动作），
不验真实视频的数值 —— 那是 `output/*/05-验收/` 与肉眼的事。

关键回归：**面积波动不适用于全身/头颈动**、**尾尖摆动是动作不是缺陷** ——
这两条以前被误判过（yawn 8.1%、lick 12.69% 均为误报），别再改回去。
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest
from PIL import Image

from photo_avatar_backend.frames.acceptance import (
    CRITERIA,
    THRESHOLDS,
    AcceptanceError,
    accept_frames,
    check_flicker,
    check_green_fringe,
    check_halo,
    check_tail,
    composite,
    load_frames,
    make_background,
    LANDING_CRITERION,
    check_landing,
    mask_iou,
)

SIZE = 48
BOX = (14, 14, 34, 34)


def frame(box=BOX, rgb=(120, 120, 120), size: int = SIZE) -> np.ndarray:
    """造一帧：中间一块 alpha=255 的实心矩形，其余透明。"""
    arr = np.zeros((size, size, 4), np.uint8)
    arr[:, :, :3] = rgb
    x0, y0, x1, y1 = box
    arr[y0:y1, x0:x1, 3] = 255
    return arr


def stacks(frames: list[np.ndarray]) -> tuple[np.ndarray, np.ndarray]:
    stack = np.stack(frames)
    return stack, stack[:, :, :, 3].astype(np.float32) / 255.0


def write_sequence(root: Path, frames: list[np.ndarray]) -> Path:
    frames_dir = root / "frames"
    frames_dir.mkdir(parents=True)
    for index, item in enumerate(frames):
        Image.fromarray(item, mode="RGBA").save(frames_dir / f"f{index:04d}.png")
    return frames_dir


# ------------------------------------------------------------------ load_frames

def test_load_frames_reads_in_filename_order(tmp_path: Path):
    # 故意乱序写入，验证读的时候按文件名排
    frames_dir = tmp_path / "frames"
    frames_dir.mkdir()
    for index in (2, 0, 1):
        Image.fromarray(frame(box=(10 + index * 4, 14, 30 + index * 4, 34)), mode="RGBA").save(
            frames_dir / f"f{index:04d}.png"
        )

    stack, alphas = load_frames(frames_dir)

    assert stack.shape == (3, SIZE, SIZE, 4)
    assert alphas.dtype == np.float32
    assert alphas.max() == 1.0
    # 第 0 帧左边界 10、第 1 帧 14、第 2 帧 18
    assert [int(np.nonzero(alphas[i, SIZE // 2] > 0.5)[0].min()) for i in range(3)] == [10, 14, 18]


def test_load_frames_rejects_directory_without_frames(tmp_path: Path):
    empty = tmp_path / "frames"
    empty.mkdir()
    with pytest.raises(AcceptanceError, match="没有帧文件"):
        load_frames(empty)


# ------------------------------------------------------------------ 1 尾巴完整

def test_tail_passes_on_a_stable_single_blob():
    _, alphas = stacks([frame() for _ in range(4)])

    result = check_tail(alphas)

    assert result["passed"] is True
    assert result["strayComponentFrameCount"] == 0
    assert result["edgeTouchFrameCount"] == 0
    assert result["maxConnectedComponents"] == 1
    assert result["foregroundAreaVariation"] == 0.0


def test_tail_flags_a_stray_component():
    broken = frame()
    broken[4:8, 4:8, 3] = 255          # 与主体分离的飞块
    _, alphas = stacks([frame(), broken, frame()])

    result = check_tail(alphas)

    assert result["passed"] is False
    assert result["strayComponentFrameCount"] == 1
    assert result["strayComponentFrames"][0]["frame"] == 1


def test_tail_flags_subject_touching_the_canvas_edge():
    touching = frame(box=(0, 14, 20, 34))   # 左边界压到 0 = 被取景框裁切
    _, alphas = stacks([frame(), touching])

    result = check_tail(alphas)

    assert result["passed"] is False
    assert result["edgeTouchFrameCount"] == 1
    assert result["edgeTouchFrames"][0]["frame"] == 1


def test_tail_flags_area_collapse():
    collapsed = frame(box=(19, 19, 29, 29))   # 面积掉到 1/4
    _, alphas = stacks([frame(), collapsed, frame()])

    result = check_tail(alphas)

    assert result["passed"] is False
    assert result["foregroundAreaVariation"] > THRESHOLDS["foregroundAreaVariation"]


def test_tail_swing_amplitude_is_diagnostic_not_a_defect():
    """尾尖大幅摆动 = 尾巴在动，不是尾巴没了。

    这个判据以前被误读成「包围盒右边界变化幅度」，摇尾视频必然假报警。
    现在它只是 `diagnostic_tailSwingAmplitude`，不参与判定。
    """
    _, alphas = stacks([
        frame(box=(4, 14, 24, 34)),
        frame(box=(14, 14, 34, 34)),
        frame(box=(24, 14, 44, 34)),
    ])

    result = check_tail(alphas)

    assert result["diagnostic_tailSwingAmplitude"] > 0.4, "摆动幅度要真的被量到"
    assert result["passed"] is True, "幅度大 ≠ 尾巴不完整"


# ------------------------------------------------------------------ 2 无绿边

def test_green_fringe_passes_on_a_neutral_subject():
    stack, alphas = stacks([frame() for _ in range(3)])

    result = check_green_fringe(stack, alphas)

    assert result["passed"] is True
    assert result["edgeGreenFringeRatioMean"] == 0.0
    assert result["interiorGreenSpillRatio"] == 0.0


def test_green_fringe_flags_a_green_subject():
    stack, alphas = stacks([frame(rgb=(120, 200, 120)) for _ in range(3)])

    result = check_green_fringe(stack, alphas)

    assert result["passed"] is False
    assert result["edgeGreenFringeRatioMean"] > THRESHOLDS["greenFringeRatio"]
    assert result["interiorGreenSpillRatio"] > THRESHOLDS["interiorSpillRatio"]


# ------------------------------------------------------------------ 3 帧间不闪烁

def test_flicker_needs_at_least_three_frames():
    stack, alphas = stacks([frame(), frame()])

    result = check_flicker(stack, alphas)

    assert result["passed"] is False
    assert "至少需要 3 帧" in result["error"]


def test_flicker_passes_on_identical_frames():
    stack, alphas = stacks([frame() for _ in range(4)])

    result = check_flicker(stack, alphas)

    assert result["passed"] is True
    assert result["silhouetteAreaJitter"] == 0.0
    assert result["alphaMassJitter"] == 0.0
    assert result["interiorAlphaStd"] == 0.0


def test_flicker_flags_an_oscillating_silhouette():
    big, small = frame(), frame(box=(19, 19, 29, 29))
    stack, alphas = stacks([big, small, big, small, big])

    result = check_flicker(stack, alphas)

    assert result["passed"] is False
    assert result["silhouetteAreaJitter"] > THRESHOLDS["silhouetteAreaJitter"]


# ------------------------------------------------------------------ 4 合成自然度

@pytest.mark.parametrize("kind", ["black", "white", "checker"])
def test_backgrounds_are_achromatic(kind: str):
    bg = make_background((SIZE, SIZE), kind)

    assert bg.shape == (SIZE, SIZE, 3)
    assert (bg[:, :, 0] == bg[:, :, 1]).all()
    assert (bg[:, :, 1] == bg[:, :, 2]).all(), "消色背景是彩度判据成立的前提"


def test_checker_background_has_two_tones():
    values = set(np.unique(make_background((SIZE, SIZE), "checker")))
    assert values == {198.0, 238.0}


def test_composite_leaves_transparent_pixels_equal_to_background():
    stack, alphas = stacks([frame()])
    bg = make_background((SIZE, SIZE), "checker")

    comp = composite(stack, alphas, "checker")

    assert (comp[0][alphas[0] == 0] == bg[alphas[0] == 0]).all()


def test_halo_passes_when_the_edge_is_clean_and_fails_on_a_colored_ring():
    stack, alphas = stacks([frame() for _ in range(3)])
    clean = check_halo(stack, alphas)
    assert clean["passed"] is True
    assert clean["worstEdgeChroma"] == 0.0

    # 外圈 3px 内塞进**半透明**绿色 = 脏边。alpha 必须 <0.5：
    # `check_halo` 的 ring 是「mask 之外」，alpha 一过 0.5 就被算进主体、那片彩度就查不到了。
    dirty = frame()
    mask = dirty[:, :, 3] > 0
    from scipy import ndimage
    ring = ndimage.binary_dilation(mask, iterations=3) & ~mask
    dirty[ring, :3] = (60, 220, 60)
    dirty[ring, 3] = 76
    stack2, alphas2 = stacks([dirty for _ in range(3)])

    assert check_halo(stack2, alphas2)["passed"] is False


# ------------------------------------------------------------------ 端到端

def test_accept_frames_writes_report_and_evidence(tmp_path: Path):
    frames_dir = write_sequence(tmp_path, [frame() for _ in range(6)])

    result = accept_frames(frames_dir, tmp_path / "out")

    assert result.overall_passed is True
    assert result.failed_criteria == ()
    assert result.frame_count == 6
    assert result.resolution == (SIZE, SIZE)

    report = json.loads(result.report_path.read_text(encoding="utf-8"))
    assert list(report) == ["framesDir", "frameCount", "resolution", "criteria", "overallPassed", "evidence"]
    assert list(report["criteria"]) == list(CRITERIA)
    assert report["overallPassed"] is True

    for key in ("checker", "black", "white", "edgeZoom", "flickerHeatmap"):
        assert result.evidence[key].endswith(".png")
        assert (result.out_dir / Path(result.evidence[key]).name).is_file()


def test_accept_frames_reports_failure_without_raising(tmp_path: Path):
    """判定不通过是**正常产出**（判据数据要留档），不是异常。旧 CLI 的约定是 rc=2。"""
    broken = frame()
    broken[4:8, 4:8, 3] = 255
    # 飞块要在每一帧都在：只出现一帧的话，「帧间不闪烁」会跟着一起 FAIL
    # （面积时间序列真的抖了）—— 那是判据正确工作，不是这里要验的东西。
    frames_dir = write_sequence(tmp_path, [broken for _ in range(4)])

    result = accept_frames(frames_dir, tmp_path / "out")

    assert result.overall_passed is False
    assert result.failed_criteria == (CRITERIA[0],)
    assert result.report_path.is_file()


def test_accept_frames_rejects_missing_frames_dir(tmp_path: Path):
    with pytest.raises(AcceptanceError, match="帧序列目录不存在"):
        accept_frames(tmp_path / "nope", tmp_path / "out")


# ------------------------------------------------------- 5-落地衔接（锚点 IoU）

def test_mask_iou_is_zero_for_two_empty_masks():
    """两张都透明 ≠ 长得一样。`0/0` 必须返回 0.0，不能返回 1.0。"""
    empty = np.zeros((4, 4), dtype=bool)

    assert mask_iou(empty, empty) == 0.0


def test_check_landing_scores_first_and_last_frames_against_the_anchor():
    """首帧管「触发跳位」、末帧管「收尾落不回」—— 两项都要过。"""
    anchor = np.zeros((SIZE, SIZE), dtype=bool)
    anchor[14:34, 14:34] = True

    # 首帧=锚点、末帧挪开一大块 ⇒ 末帧 IoU 低
    good_then_bad = np.stack([
        frame(box=(14, 14, 34, 34))[:, :, 3] >= 128,
        frame(box=(26, 26, 44, 44))[:, :, 3] >= 128,
    ])
    result = check_landing(good_then_bad, anchor=anchor)

    assert result["passed"] is False
    assert result["firstFrameIou"] == pytest.approx(1.0)
    assert result["lastFrameIou"] < 0.88


def test_check_landing_passes_when_both_ends_match_the_anchor():
    anchor = np.zeros((SIZE, SIZE), dtype=bool)
    anchor[14:34, 14:34] = True
    same = np.stack([frame()[:, :, 3] >= 128 for _ in range(3)])

    result = check_landing(same, anchor=anchor)

    assert result["passed"] is True
    assert result["firstFrameIou"] == pytest.approx(1.0)
    assert result["lastFrameIou"] == pytest.approx(1.0)


def test_check_landing_does_not_judge_without_an_anchor():
    """缺参考 ≠ 不合格：没有锚点时必须返回 `passed=None`，否则引擎侧会误报 FAIL。"""
    result = check_landing(np.stack([frame()[:, :, 3] >= 128]), anchor=None)

    assert result["passed"] is None
    assert "不判" in result["note"]


def test_check_tail_attributes_edge_touching_to_the_offending_edges():
    """贴边的**归因**：上/下多为起跳落地超框、左/右多为横摆超框。

    两者处置不同 —— 前者看取景（`matting` 自动 refit），后者要收紧横向动作幅度。
    """
    # 造一串底部贴边的帧：主体下沿压到画布最后一行
    touching = np.stack([frame(box=(14, 14, 34, SIZE))[:, :, 3] >= 128 for _ in range(4)])
    result = check_tail(touching.astype(np.float32))

    attribution = result["edgeTouchAttribution"]
    assert result["edgeTouchFrameCount"] > 0
    assert attribution is not None
    assert attribution["worstEdge"] == "下"
    assert attribution["edgeHitFrameCounts"]["下"] == result["edgeTouchFrameCount"]
    assert attribution["touchFrameRatio"] == pytest.approx(
        result["edgeTouchFrameCount"] / 4, abs=1e-4
    )


def test_check_tail_leaves_attribution_empty_when_nothing_touches_an_edge():
    clean = np.stack([frame()[:, :, 3] >= 128 for _ in range(3)])

    result = check_tail(clean.astype(np.float32))

    assert result["edgeTouchFrameCount"] == 0
    assert result["edgeTouchAttribution"] is None, "没贴边就不该有归因块"


def test_accept_frames_adds_the_landing_criterion_only_when_an_anchor_is_given(tmp_path: Path):
    """不做动作的包（idle 单支）**不**多出这一条 —— 自比没意义。"""
    frames_dir = write_sequence(tmp_path, [frame() for _ in range(4)])

    without = accept_frames(frames_dir, tmp_path / "out-plain")
    assert LANDING_CRITERION not in without.criteria
    assert list(without.criteria) == list(CRITERIA)

    with_anchor = accept_frames(
        frames_dir, tmp_path / "out-anchor", anchor_frames_dir=frames_dir
    )
    assert LANDING_CRITERION in with_anchor.criteria
    assert with_anchor.criteria[LANDING_CRITERION]["passed"] is True


def test_acceptance_error_is_a_normal_exception():
    """`SystemExit` 不是 `Exception`，job_store 的 `except Exception` 抓不到 —— 会静默逃逸。"""
    assert not issubclass(AcceptanceError, SystemExit)
    assert issubclass(AcceptanceError, Exception)

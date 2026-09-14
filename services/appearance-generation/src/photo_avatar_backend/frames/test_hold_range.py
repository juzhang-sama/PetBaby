# -*- coding: utf-8 -*-
"""「悬空保持段」体检的测试。

这个模块的契约是**反直觉**的，所以测试也照那个来：

- 它**不给结论** —— 报告里的候选段与 IoU 都是**给人看的事实**；
- **不因为 IoU 低就判失败**（05 的出厂值 `[30,59]` 的 IoU 只有 0.41，
  用「IoU 高」当判据会把正确值拒掉）；
- 值只从**动作配置**来（`action_hold_range`），没有就是不写这个键。
"""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest
from PIL import Image

from photo_avatar_backend.frames.action_prompts import ActionPromptError, action_hold_range
from photo_avatar_backend.frames.hold_range import (
    LOOP_IOU_REFERENCE,
    inspect_hold_range,
)

# 真实资产：内置 05 的拎起（81 帧）。仓库里就在 `apps/desktop/public/builtin-pets/`。
REAL_GRAB = (
    Path(__file__).resolve().parents[5]
    / "apps/desktop/public/builtin-pets/05-silver-tabby/frames/grab-release"
)


def write_action_frames(root: Path, *, count: int, narrow: range) -> Path:
    """造一段帧序列：`narrow` 那几帧的主体明显更窄（模拟「被拎起来」）。"""
    frames = root / "frames"
    frames.mkdir(parents=True)
    for index in range(count):
        width = 40 if index in narrow else 100
        image = Image.new("RGBA", (160, 120), (0, 0, 0, 0))
        block = np.zeros((120, width, 4), dtype=np.uint8)
        block[:, :, 3] = 255
        image.paste(Image.fromarray(block), (60 - width // 2, 20))
        image.save(frames / f"f{index:04d}.png")
    return frames


def test_the_candidate_is_the_longest_narrow_run(tmp_path: Path):
    frames = write_action_frames(tmp_path, count=20, narrow=range(8, 15))

    report = inspect_hold_range(frames, log=lambda _: None)

    assert report.frame_count == 20
    assert report.candidate is not None
    assert report.candidate.span == (8, 14)
    assert report.candidate.frame_count == 7
    assert report.narrowest_width == 40


def test_a_configured_window_is_reported_but_never_judged(tmp_path: Path):
    """配置的窗口只被**体检**，不被打分 —— 03 的出厂值 IoU 只有 0.41 却是对的。"""
    frames = write_action_frames(tmp_path, count=20, narrow=range(8, 15))

    report = inspect_hold_range(frames, configured=(0, 19), log=lambda _: None)

    assert report.configured == (0, 19), "配置值原样带出来，不做修正"
    # 0..19 里既有宽也有窄 → 首尾不像 → IoU 低。**这不是失败**，只是事实。
    assert report.candidate is not None and report.candidate.span == (8, 14)


def test_an_out_of_range_configured_window_is_ignored_with_a_warning(tmp_path: Path):
    frames = write_action_frames(tmp_path, count=10, narrow=range(3, 6))
    lines: list[str] = []

    report = inspect_hold_range(frames, configured=(5, 99), log=lines.append)

    assert report.configured is None, "越界就不带出来（别让它进 manifest）"
    assert any("越界" in line for line in lines)


def test_constant_width_makes_the_whole_clip_the_candidate(tmp_path: Path):
    """宽度没有变化时「最窄段」退化成全长 —— 事实如此，照报（不假装找到了什么）。"""
    frames = write_action_frames(tmp_path, count=6, narrow=range(0))

    report = inspect_hold_range(frames, log=lambda _: None)

    assert report.candidate is not None
    assert report.candidate.span == (0, 5)


def test_the_report_says_what_happens_when_nothing_is_configured(tmp_path: Path):
    frames = write_action_frames(tmp_path, count=8, narrow=range(2, 5))

    report = inspect_hold_range(frames, log=lambda _: None)
    text = "\n".join(report.log_lines())

    assert "没有 holdRange" in text
    assert "安全降级" in text


def test_a_missing_frames_directory_is_rejected(tmp_path: Path):
    with pytest.raises(ValueError, match="does not exist"):
        inspect_hold_range(tmp_path / "nope", log=lambda _: None)


# ------------------------------------------------------------------ 配置解析


def test_hold_range_is_absent_by_default():
    assert action_hold_range({"actionId": "grab-release"}) is None


def test_a_configured_hold_range_is_returned_as_a_tuple():
    assert action_hold_range({"holdRange": [30, 59]}) == (30, 59)


@pytest.mark.parametrize(
    "raw",
    [
        [30],            # 长度不对
        [30, 59, 60],
        "30-59",         # 类型不对
        [30.5, 59],      # 不是整数
        [True, 59],      # bool 是 int 的子类，必须挡掉
        [-1, 59],        # 负下标
        [59, 30],        # lo > hi
    ],
)
def test_a_malformed_hold_range_is_rejected(raw: object):
    with pytest.raises(ActionPromptError, match="holdRange"):
        action_hold_range({"holdRange": raw})


# ------------------------------------------------------------------ 真实资产


@pytest.mark.skipif(not REAL_GRAB.is_dir(), reason="仓库里没有内置 05 的拎起帧")
def test_the_real_builtin_grab_release_pins_the_41_percent_finding():
    """钉住那个**让自动标定站不住**的事实。

    05 出厂写的是 `[30,59]`，而它的循环 IoU 只有 **0.41** —— 远低于参考线 0.9。
    如果哪天有人「顺手」把 `LOOP_IOU_REFERENCE` 变成闸门，这条会告诉他：
    那会把一个**正确的出厂值**拒掉（而这正是本模块只体检、不判定的原因）。
    """
    report = inspect_hold_range(REAL_GRAB, configured=(30, 59), log=lambda _: None)

    assert report.frame_count == 81
    assert report.configured == (30, 59)
    assert report.candidate is not None, "真实资产里必须能找到候选段"
    assert report.configured_loop_iou == pytest.approx(0.41, abs=0.02)
    assert report.configured_loop_iou < LOOP_IOU_REFERENCE, (
        "出厂值的 IoU 低于参考线 —— 所以参考线只能是「参考」，不能当闸门"
    )

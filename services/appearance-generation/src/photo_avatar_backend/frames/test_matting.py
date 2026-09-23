"""抠像测试。

只测**不依赖 ffmpeg / 不依赖真实视频**的部分：入参校验、色键数学、自动取景、
形态学清理，以及「错误必须能被服务侧抓到」。

帧内容本身靠黄金回归保证（同一支视频重跑 → 逐帧 sha256 一致），不在这里重复。
"""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from photo_avatar_backend.frames.matting import (
    CROP_MARGIN,
    MattingError,
    box_contains,
    chroma_alpha,
    clean_mask,
    compute_autocrop,
    match_color,
    matte_video,
    plan_crop,
    rvm_alpha,
    sam2_alpha,
)


def green_frame(height: int, width: int) -> np.ndarray:
    return np.full((height, width, 3), (0, 255, 0), dtype=np.uint8)


def test_matting_error_is_a_value_error_so_the_service_can_catch_it():
    """`SystemExit` **不是** `Exception`。

    `job_store.run_reserved` 用 `except Exception` 兜 runner 异常；抛 SystemExit
    会在 worker 线程里静默逃逸 —— 表现为「job 永远 running」，日志里什么都没有。
    """
    assert issubclass(MattingError, ValueError)
    assert issubclass(MattingError, Exception)
    assert not issubclass(MattingError, SystemExit)


def test_unimplemented_methods_raise_instead_of_exiting():
    blank = [np.zeros((4, 4, 3), dtype=np.uint8)]

    with pytest.raises(MattingError):
        rvm_alpha(blank)
    with pytest.raises(MattingError):
        sam2_alpha(blank)


def test_chroma_alpha_is_backgroundness_not_foregroundness():
    """返回的是「像绿幕的程度」：纯绿 -> 1.0；中性灰 -> 0.0。

    名字容易看反，而 compute_autocrop 正是用 `chroma_alpha(rgb) <= 0.5` 当前景。
    """
    assert np.allclose(chroma_alpha(green_frame(8, 8)), 1.0)
    assert np.allclose(chroma_alpha(np.full((8, 8, 3), (128, 128, 128), dtype=np.uint8)), 0.0)


def test_chroma_alpha_ramps_linearly_across_the_key_band():
    """KEY_LOW=18 / KEY_HIGH=62，所以绿色超出量 40 应落在软过渡中点 0.5。"""
    exact_mid = np.array([[[0, 40, 0]]], dtype=np.uint8)

    assert chroma_alpha(exact_mid)[0, 0] == pytest.approx(0.5, abs=1e-6)


def test_autocrop_square_covers_the_union_of_every_frame():
    height = width = 200
    frames = []
    for offset in (0, 30):
        frame = green_frame(height, width)
        frame[50:120, 40 + offset:140 + offset] = (200, 200, 200)
        frames.append(frame)

    (x, y, side), (fx0, fy0, fx1, fy1) = compute_autocrop(frames)

    assert (fx0, fy0) == (40, 50)
    assert (fx1, fy1) == (140 + 30 - 1, 119)
    assert side <= min(height, width)
    # 正方形必须装下并集，否则会切掉甩出画幅的尾巴
    assert x <= fx0 and y <= fy0 and x + side > fx1 and y + side > fy1
    # 紧裁：边长应等于并集长边 + 两侧硬边距（不超过画幅）
    assert side == min(min(height, width), max(fx1 - fx0 + 1, fy1 - fy0 + 1) + 2 * CROP_MARGIN)


def block_frames(size: int, boxes: list[tuple[int, int, int, int]]) -> list[np.ndarray]:
    """画 `size×size` 的绿幕帧，每帧放一个灰色方块（闭区间坐标）。"""
    frames = []
    for x0, y0, x1, y1 in boxes:
        frame = green_frame(size, size)
        frame[y0:y1 + 1, x0:x1 + 1] = (200, 200, 200)
        frames.append(frame)
    return frames


def test_box_contains_counts_the_far_edge_inclusively():
    """框内最右/最下列是 `x + side - 1`，差 1 就漏判一整列像素。"""
    assert box_contains((0, 0, 100), (0, 0, 99, 99)) is True
    assert box_contains((0, 0, 100), (0, 0, 100, 99)) is False
    assert box_contains((10, 10, 100), (10, 10, 109, 109)) is True


def test_plan_crop_reuses_the_reference_box_when_the_foreground_fits():
    """装得进 → 原样复用（旧行为逐字节不变，触发瞬间零跳位）。"""
    frames = block_frames(200, [(20, 20, 59, 59)])

    plan = plan_crop(frames, autocrop=False, crop_box=(10, 10, 100))

    assert plan.source == "reused"
    assert plan.box == (10, 10, 100)
    assert plan.clipped_by_reference is False
    assert plan.refit_target == 0


def test_plan_crop_refits_with_its_own_box_when_the_reference_would_clip():
    """装不进 → 退回动作自己的并集取景，再缩回参考框边长：**一个像素都不裁**。

    毛球2 的 `grab-release` 就是这条路径：并集 634×614 装不进 idle 的 564 框，
    硬复用的结果是落地那一帧的脚被整条切掉。
    """
    frames = block_frames(200, [(0, 0, 159, 159)])

    plan = plan_crop(frames, autocrop=False, crop_box=(0, 0, 120))

    assert plan.source == "refit"
    assert plan.clipped_by_reference is True
    assert plan.reference == (0, 0, 120)
    assert plan.refit_target == 120, "缩回参考框边长，画布才和 idle 一致"
    side = plan.box[2]
    assert side > 120 and plan.refit_target / side < 1, "超框必然是缩小，不是放大"
    assert box_contains(plan.box, plan.union), "重取景后不该还裁前景"


def test_plan_crop_reports_clipping_when_no_square_can_cover_the_foreground():
    """画幅太扁、正方形装不下全部前景时，`clipsForeground` 必须如实为 True。"""
    frames = []
    for _ in range(2):
        frame = np.full((100, 200, 3), (0, 255, 0), dtype=np.uint8)
        frame[0:100, 0:200] = (200, 200, 200)
        frames.append(frame)

    plan = plan_crop(frames, autocrop=True, crop_box=None)

    assert plan.source == "auto"
    assert plan.box[2] == 100, "正方形最大只能取到短边"
    assert not box_contains(plan.box, plan.union), "并集 200 宽，100 的方框装不下"


def test_plan_crop_flags_an_upscale_when_the_source_canvas_is_smaller_than_the_box():
    """源画布比参考框还小 ⇒ 缩回参考框那一步是**放大**，必须能被识别出来。

    没有新像素，只有 LANCZOS 插值 ⇒ 糊，且不可逆。数学上无解（要么裁、要么糊），
    选糊但要在日志和 `crop` 记录里吼一声。
    """
    # 源只有 80×80，而参考框要 120（模拟「小画布宠 + 大参考框」）
    frames = block_frames(80, [(10, 10, 69, 69)])

    plan = plan_crop(frames, autocrop=False, crop_box=(0, 0, 80))

    # 并集 60×60 装得进 80 的框 ⇒ 走复用，不放大
    assert plan.source == "reused"
    assert plan.refit_upscales is False


def test_plan_crop_marks_upscale_only_when_refit_actually_enlarges():
    """`refit_upscales` 只在重取景的框**小于**参考框时为真（即真的在放大）。"""
    # 并集 160 宽（wanted = 160+2*12 = 184）装不进 100 的框 ⇒ refit，且框(184) > 参考(100) ⇒ 缩小
    small = plan_crop(block_frames(200, [(0, 0, 159, 159)]),
                      autocrop=False, crop_box=(0, 0, 100))
    assert small.source == "refit"
    assert small.box[2] > small.refit_target
    assert small.refit_upscales is False

    # 并集 0..159（宽 160）装得进 160 的框（闭区间刚好放下）⇒ 复用，不是 refit。
    exact = plan_crop(block_frames(200, [(0, 0, 159, 159)]),
                      autocrop=False, crop_box=(0, 0, 160))
    assert exact.source == "reused", "闭区间刚好装下就该复用（差 1 会误判成 refit）"

    # 并集 0..160（宽 161）装不进 160 ⇒ refit；autocrop 取 wanted = 161+24 = 185 > 160
    # ⇒ 仍是缩小。用来确认「refit 不等于放大」。
    shrink = plan_crop(block_frames(200, [(0, 0, 160, 160)]),
                       autocrop=False, crop_box=(0, 0, 160))
    assert shrink.source == "refit"
    assert shrink.box[2] > shrink.refit_target
    assert shrink.refit_upscales is False, "取了更大的框再缩回小框 = 缩小"

    # 真正的放大：并集顶满小画布，autocrop 只能取到短边，缩回参考框才是放大。
    # 直接验证"框 < target"这个判据本身（端到端构造不出来：同源必同画布）。
    from photo_avatar_backend.frames.matting import CropPlan
    assert CropPlan((0, 0, 60), (0, 0, 59, 59), "refit", (0, 0, 80), True, 80).refit_upscales
    assert not CropPlan((0, 0, 60), (0, 0, 59, 59), "refit", (0, 0, 60), True, 60).refit_upscales
    assert not CropPlan(None, (0, 0, 9, 9), "off").refit_upscales, "没框就不谈放大"


def test_plan_crop_keeps_the_reference_box_when_there_is_no_foreground():
    """没有前景时并集是空区间 —— 别拿它去触发重取景（会算出负边长）。"""
    plan = plan_crop([green_frame(80, 80)], autocrop=False, crop_box=(5, 5, 40))

    assert plan.source == "reused"
    assert plan.box == (5, 5, 40)
    assert plan.refit_upscales is False


def test_plan_crop_rejects_a_reference_box_outside_the_source():
    frames = block_frames(80, [(10, 10, 20, 20)])

    with pytest.raises(MattingError, match="does not fit inside"):
        plan_crop(frames, autocrop=False, crop_box=(50, 0, 40))


def test_plan_crop_reuse_is_bit_identical_to_the_old_hard_reuse():
    """**老资产零回归**：并集装得进参考框时，取景结果必须与旧行为（硬复用）完全一致。

    旧代码就是 `crop_box=...` 直接切那个框。新代码走 `reused` 分支返回同一个三元组，
    所以「库里的资产重打包」不会因为这次改动产生任何像素差异 —— 这是那条承诺的机械证明。
    """
    frames = block_frames(200, [(40, 40, 120, 120)])      # 并集 81×81，落在框 (10,10,160) 内

    plan = plan_crop(frames, autocrop=False, crop_box=(10, 10, 160))

    assert plan.source == "reused"
    assert plan.box == (10, 10, 160), "装得下时用的必须还是那个参考框，一个像素都不挪"
    assert plan.clipped_by_reference is False
    assert plan.refit_upscales is False


def test_plan_crop_takes_over_only_when_the_reference_would_clip():
    """边界另一侧：并集只要**越出参考框一列**，就必须让位给动作自己的框。

    与上一条成对 —— 一侧逐字节不变、另一侧才启用新行为，这就是改动的作用域边界。
    """
    frames = block_frames(200, [(40, 40, 170, 120)])      # 右沿 170 > 10+160-1

    plan = plan_crop(frames, autocrop=False, crop_box=(10, 10, 160))

    assert plan.source == "refit"
    assert plan.box != (10, 10, 160)
    assert plan.clipped_by_reference is True
    assert box_contains(plan.box, plan.union)


def test_clean_mask_keeps_the_largest_component_and_suppresses_specks():
    alpha = np.zeros((40, 40), dtype=np.float32)
    alpha[10:30, 10:30] = 1.0     # 主体
    alpha[2:4, 2:4] = 1.0         # 背景里的孤立小点

    cleaned = clean_mask(alpha)

    assert cleaned[20, 20] >= 0.9            # 主体保住
    assert cleaned[2, 2] < 0.5               # 噪点被压低
    assert cleaned.min() >= 0.0 and cleaned.max() <= 1.0


def test_clean_mask_leaves_an_empty_mask_untouched():
    empty = np.zeros((8, 8), dtype=np.float32)

    assert np.array_equal(clean_mask(empty), empty)


@pytest.mark.parametrize(
    ("kwargs", "match"),
    [
        ({"method": "magic"}, "unsupported matting method"),
        ({"size": -1}, "non-negative"),
        ({"temporal": -1}, "non-negative"),
        ({"spatial": -1.0}, "non-negative"),
        ({"warmup_tol": -1.0}, "non-negative"),
    ],
)
def test_rejects_invalid_arguments_before_touching_ffmpeg(tmp_path: Path, kwargs, match):
    video = tmp_path / "video.mp4"
    video.write_bytes(b"not a real video")

    with pytest.raises(ValueError, match=match):
        matte_video(video, tmp_path / "out", **kwargs)


def test_rejects_a_missing_video(tmp_path: Path):
    with pytest.raises(ValueError, match="video does not exist"):
        matte_video(tmp_path / "nope.mp4", tmp_path / "out")


def test_rejects_a_missing_color_match_master(tmp_path: Path):
    video = tmp_path / "video.mp4"
    video.write_bytes(b"not a real video")

    with pytest.raises(ValueError, match="color-match master does not exist"):
        matte_video(video, tmp_path / "out", color_match=tmp_path / "nope.png")


def _color_ref(rgb: np.ndarray) -> np.ndarray:
    return np.stack([rgb.mean(axis=0), rgb.std(axis=0)])


def test_match_color_pulls_the_core_mean_onto_the_master():
    """核心均值被搬到母版均值上（校色的本职）。

    无噪声输入 ⇒ `src_std == dst_std` ⇒ `gain` 退化成 1（纯平移），结果应精确落到母版统计量。
    """
    rgb = np.full((40, 40, 3), (200.0, 184.0, 182.0), dtype=np.float32)
    alpha = np.ones((40, 40), dtype=np.float32)
    ref = np.stack([np.array([222.0, 216.0, 209.0], np.float32), np.full(3, 8.0, np.float32)])

    out = match_color(rgb, alpha, ref)

    assert np.allclose(out.mean(axis=(0, 1)), (222.0, 216.0, 209.0), atol=1e-3)


def test_match_color_reduces_distance_to_the_master_on_noisy_input():
    """有噪声（`gain != 1`）时也应当把核心拉近母版 —— 允许被绿余量闸口削掉一点。"""
    rng = np.random.default_rng(0)
    dark = np.clip(rng.normal((200.0, 184.0, 182.0), 10.0, (64, 64, 3)), 0, 255).astype(np.float32)
    alpha = np.ones((64, 64), dtype=np.float32)
    master = np.clip(rng.normal((222.0, 216.0, 209.0), 20.0, (64, 64, 3)), 0, 255).astype(np.float32)
    flat = master.reshape(-1, 3)
    target = flat.mean(axis=0)

    out = match_color(dark, alpha, np.stack([target, flat.std(axis=0)]))

    before = float(np.linalg.norm(dark.reshape(-1, 3).mean(axis=0) - target))
    after = float(np.linalg.norm(out.reshape(-1, 3).mean(axis=0) - target))
    assert after < before * 0.35


def test_match_color_never_increases_green_excess():
    """校色**不许**把任一像素的绿余量抬到校色前之上。

    历史 bug：线性变换逐通道各拉各的，把 `despill` 刚压下去的半透明边缘带重新推绿
    —— 实测 Mini 档 `edgeGreenFringeRatio` 3e-06 → 0.1298（阈 0.02），「无绿边」判据直接 FAIL。
    """
    # 左半：被整体压暗的主体（提供 core 统计量，且自身不绿）；
    # 右半：带绿残留的半透明边缘带（alpha=0.6，不进 core 统计）。
    rgb = np.full((64, 64, 3), (200.0, 184.0, 182.0), dtype=np.float32)
    rgb[:, 48:] = (150.0, 190.0, 148.0)
    alpha = np.ones((64, 64), dtype=np.float32)
    alpha[:, 48:] = 0.6
    ref = np.stack([np.array([222.0, 216.0, 209.0], np.float32), np.full(3, 12.0, np.float32)])

    before = rgb[:, :, 1] - np.maximum(rgb[:, :, 0], rgb[:, :, 2])
    out = match_color(rgb, alpha, ref)
    after = out[:, :, 1] - np.maximum(out[:, :, 0], out[:, :, 2])

    assert np.all(after <= np.maximum(before, 0.0) + 1e-4)
    # 闸口只削"被推绿"的部分，不能把校色本身也削掉：主体仍然变亮
    assert out[:, :48, 1].mean() > rgb[:, :48, 1].mean() + 20


def test_match_color_leaves_a_tiny_frame_untouched():
    """核心像素太少时不做任何事（避免用几个点估出来的统计量乱拉整帧）。"""
    rgb = np.full((4, 4, 3), (10.0, 200.0, 10.0), dtype=np.float32)
    alpha = np.zeros((4, 4), dtype=np.float32)

    out = match_color(rgb, alpha, np.stack([np.full(3, 100.0, np.float32),
                                            np.full(3, 10.0, np.float32)]))

    assert np.array_equal(out, rgb)

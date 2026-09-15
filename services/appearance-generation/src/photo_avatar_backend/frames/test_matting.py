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
    chroma_alpha,
    clean_mask,
    compute_autocrop,
    match_color,
    matte_video,
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

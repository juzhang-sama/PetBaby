# -*- coding: utf-8 -*-
"""串链（`packFrameSequence`）测试。

绝大部分是纯逻辑：zip 的**字节确定性**、入参闸口、包根校验。
唯一的真端到端用 ffmpeg 现场合成一支 128×128 的假绿幕视频（**不烧算力**），
没有 ffmpeg 就 skip —— 真视频的通过记录在 `output/一键出宠-测试-2026-09-13/`。
"""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import zipfile
from pathlib import Path

import numpy as np
import pytest
from PIL import Image, ImageDraw

from photo_avatar_backend.frames.pipeline import (
    MANIFEST_NAME,
    build_frame_sequence,
    zip_package,
)

FFMPEG = shutil.which("ffmpeg")


def make_package(root: Path, frames: int = 3) -> Path:
    """造一个最小的「运行时包」目录树（内容随便，只验 zip 行为）。"""
    package = root / "10-运行时包"
    (package / "frames" / "idle-combo").mkdir(parents=True)
    for index in range(frames):
        (package / "frames" / "idle-combo" / f"f{index:04d}.webp").write_bytes(
            b"RIFF....WEBPVP8 " + bytes([index]) * 16
        )
    (package / MANIFEST_NAME).write_text(
        json.dumps({"schemaVersion": 7}, ensure_ascii=False), encoding="utf-8"
    )
    return package


# ------------------------------------------------------------------ zip

def test_zip_packs_the_package_tree_at_the_zip_root(tmp_path: Path):
    package = make_package(tmp_path)

    zip_path, size = zip_package(package, tmp_path / "out.zip")

    with zipfile.ZipFile(zip_path) as archive:
        names = sorted(archive.namelist())
    assert names == [
        "frames/idle-combo/f0000.webp",
        "frames/idle-combo/f0001.webp",
        "frames/idle-combo/f0002.webp",
        "manifest.json",
    ], "解开 zip 就该是包根的内容，不能多一层目录"
    assert size == zip_path.stat().st_size
    with zipfile.ZipFile(zip_path) as archive:
        assert archive.read(MANIFEST_NAME) == (package / MANIFEST_NAME).read_bytes()


def test_zip_bytes_are_deterministic(tmp_path: Path):
    """同一份内容必须产同一串字节 —— 否则「重跑比对 zip」根本没法做。

    zip 会把源文件 mtime 写进条目头，所以实现里把时间戳固定成了常数。
    """
    package = make_package(tmp_path)

    first, _ = zip_package(package, tmp_path / "a.zip")
    second, _ = zip_package(package, tmp_path / "b.zip")

    assert first.read_bytes() == second.read_bytes()


def test_zip_rejects_package_without_manifest(tmp_path: Path):
    package = make_package(tmp_path)
    (package / MANIFEST_NAME).unlink()

    with pytest.raises(ValueError, match="no manifest.json"):
        zip_package(package, tmp_path / "out.zip")


# ------------------------------------------------------------------ 入参闸口

def test_build_rejects_missing_video(tmp_path: Path):
    with pytest.raises(ValueError, match="video does not exist"):
        build_frame_sequence(
            tmp_path / "nope.mp4", tmp_path / "out", pet_id="p", display_name="n"
        )


def test_build_rejects_non_empty_output_directory(tmp_path: Path):
    """中间产物上百个文件，原地重跑要批量删除会被守卫拦 —— 必须换新目录。"""
    video = tmp_path / "v.mp4"
    video.write_bytes(b"\x00")
    out = tmp_path / "out"
    out.mkdir()
    (out / "stale.txt").write_text("旧产物", encoding="utf-8")

    with pytest.raises(ValueError, match="must be empty"):
        build_frame_sequence(video, out, pet_id="p", display_name="n")

    assert (out / "stale.txt").exists()


# ------------------------------------------------------------------ 端到端

def synth_green_video(path: Path, *, size: int = 128, frames: int = 8, fps: int = 8) -> Path:
    """现场合成一支「纯绿背景 + 移动色块」的 mp4（**不烧算力**）。

    逐帧画 PNG 再用 ffmpeg 合成，而不是让 `lavfi` 的 `drawbox` 画：
    实测这台机器上 `-vf drawbox=...` 一帧都没画上去 —— 抽出来的帧 100% 是纯绿
    (0,254,0)，前景像素 0。夹具必须确定性，不能靠 filter 表达式的运气。

    色块取 (136,68,34)：`G - max(R,B) = -68`，远低于 `KEY_LOW`，必被判为前景。
    """
    sequence = path.parent / f"{path.stem}-seq"
    sequence.mkdir(parents=True, exist_ok=True)
    block = (136, 68, 34)
    for index in range(frames):
        image = Image.new("RGB", (size, size), (0, 255, 0))
        left = 10 + index * 6
        ImageDraw.Draw(image).rectangle([left, 30, left + 40, 90], fill=block)
        image.save(sequence / f"f{index:04d}.png")
    subprocess.run(
        [FFMPEG, "-y", "-hide_banner", "-loglevel", "error",
         "-framerate", str(fps), "-i", str(sequence / "f%04d.png"),
         "-c:v", "libx264", "-crf", "0", "-pix_fmt", "yuv420p",
         str(path)],
        check=True, capture_output=True,
    )
    return path


@pytest.mark.skipif(FFMPEG is None, reason="需要 ffmpeg 现场合成测试视频")
def test_build_end_to_end_from_a_synthetic_video(tmp_path: Path):
    video = synth_green_video(tmp_path / "green.mp4")

    result = build_frame_sequence(
        video,
        tmp_path / "out",
        pet_id="99-synth",
        display_name="合成测试（短毛猫）",
        fps=8.0,
        frame_duration_ms=42,
    )

    # 交付物
    assert result.zip_path.is_file()
    assert result.zip_path.name == "99-synth.zip"
    assert result.zip_bytes > 0
    assert result.frame_format == "webp"

    # 中间产物都留档
    assert (result.out_dir / "04-抠像" / "抠像参数.json").is_file()
    assert (result.out_dir / "05-验收" / "验收报告.json").is_file()
    assert result.manifest_path.is_file()

    # manifest 是 schema 7 且指向 .webp
    manifest = json.loads(result.manifest_path.read_text(encoding="utf-8"))
    assert manifest["schemaVersion"] == 7
    assert manifest["renderer"] == "frame-sequence-v1"
    assert manifest["petId"] == "99-synth"
    assert manifest["baseImage"].endswith(".webp")
    assert all(item.endswith(".webp") for item in manifest["actions"][0]["frames"])
    assert len(manifest["actions"][0]["frames"]) == result.frame_count

    # 帧源目录里还是 PNG（审计留档），包里才是 WebP
    png_frames = sorted((result.out_dir / "04-抠像" / "frames").glob("f*.png"))
    assert len(png_frames) == result.frame_count
    assert not list(result.package_dir.rglob("*.png")), "包里不该混 PNG（否则 zip 直接胖 10 倍）"
    # 「WebP 确实更小」不在合成色块上断言：纯色图的 PNG 已经压到极小，
    # WebP 的容器开销反而更大（实测 5338 vs 3947）。体积结论由真实帧支撑
    # （614×614 × 288 帧：PNG 72.6 MB → WebP 9.7 MB），见 test_packing 的同名说明。

    # zip 里的帧必须与 manifest 的 sha256 对得上 —— 这是 Rust 安装时会校验的东西
    with zipfile.ZipFile(result.zip_path) as archive:
        assert sorted(archive.namelist()) == sorted(
            [MANIFEST_NAME] + [entry["relativePath"] for entry in manifest["files"]]
        )
        for entry in manifest["files"]:
            data = archive.read(entry["relativePath"])
            assert hashlib.sha256(data).hexdigest() == entry["sha256"]
        assert archive.read(MANIFEST_NAME) == result.manifest_path.read_bytes()

    # 验收结论自洽：FAIL 也照样出包（四判据是排雷，不是闸门）
    assert result.overall_passed == result.acceptance.overall_passed
    assert result.failed_criteria == result.acceptance.failed_criteria
    assert set(result.acceptance.criteria) == {
        "1-尾巴完整", "2-无绿边", "3-帧间不闪烁", "4-黑白棋盘格自然",
    }


@pytest.mark.skipif(FFMPEG is None, reason="需要 ffmpeg 现场合成测试视频")
def test_build_acceptance_failure_still_produces_the_package(tmp_path: Path):
    """判据 FAIL 时**不抛异常**：老王定的形态是「跑完弹预览、用户确认才安装」。"""
    video = synth_green_video(tmp_path / "green.mp4", frames=4, fps=8)

    result = build_frame_sequence(
        video, tmp_path / "out", pet_id="99-synth", display_name="n", fps=8.0
    )

    assert result.zip_path.is_file()
    if not result.overall_passed:
        assert result.failed_criteria, "FAIL 必须能说出是哪条判据"

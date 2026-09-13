"""写实风 `packFrameSequence` 的服务侧装配测试。

端到端那条用 ffmpeg 现场合成一支 128×128 的假绿幕视频（**不烧算力**），
没有 ffmpeg 就 skip。合成夹具与 `frames/test_pipeline.py` 里那份是**有意重复**的：
测试夹具不跨模块借 —— 否则一边改了夹具，另一边的失败信息会变得莫名其妙。
"""

from __future__ import annotations

import hashlib
import io
import json
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

import pytest
from PIL import Image, ImageDraw

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from photo_avatar_backend.contracts import FrameStepRequest  # noqa: E402
from photo_avatar_backend.frame_pipeline import (  # noqa: E402
    FramePipelineError,
    FrameSequenceArtifact,
    generate_motion_source,
    motion_source_path,
    pack_frame_sequence,
    scratch_dir,
)

FFMPEG = shutil.which("ffmpeg")


def _request(**overrides: object) -> FrameStepRequest:
    payload: dict[str, object] = {
        "session_id": "desktop-session-1",
        "revision": 1,
        "provider_session_id": "provider-1",
        "step": "packFrameSequence",
        "attempt": 1,
        "consent_version": "photo-avatar-third-party-ai-lk888-no-delete-v2",
        "source_images": (),
        "pet_id": "09-newcat",
        "display_name": "我的猫",
        "species": "cat",
    }
    payload.update(overrides)
    return FrameStepRequest(**payload)


# ------------------------------------------------------------------ 路径与闸口


def test_scratch_paths_are_keyed_by_provider_session(tmp_path: Path):
    """mp4 的 key 是 **providerSessionId 而不是 jobId**。

    同一个 providerSession 重试要命中同一个文件，才谈得上「复用不重付 5.69 算力」。
    """
    assert scratch_dir(tmp_path, "provider-1") == tmp_path / "scratch" / "provider-1"
    assert motion_source_path(tmp_path, "provider-1") == (
        tmp_path / "scratch" / "provider-1" / "motion-source.mp4"
    )


def test_missing_motion_source_is_reported_clearly(tmp_path: Path):
    with pytest.raises(FramePipelineError, match="motion source video is missing"):
        pack_frame_sequence(_request(), state_dir=tmp_path)


def test_the_pack_step_refuses_another_step(tmp_path: Path):
    with pytest.raises(FramePipelineError, match="got step"):
        pack_frame_sequence(_request(step="generateMotionSource"), state_dir=tmp_path)


def test_the_pack_step_refuses_a_missing_provider_session(tmp_path: Path):
    with pytest.raises(FramePipelineError, match="requires a providerSessionId"):
        pack_frame_sequence(_request(provider_session_id=None), state_dir=tmp_path)


def test_generate_motion_source_reports_that_it_is_not_implemented(tmp_path: Path):
    """还没实现（提示词契约化是下一片）。消息里要指明**卡在哪**，别只说「失败」。"""
    with pytest.raises(FramePipelineError, match="not implemented yet"):
        generate_motion_source(_request(step="generateMotionSource"), state_dir=tmp_path)


# ------------------------------------------------------------------ wire 形状


def test_artifact_wire_carries_the_acceptance_verdict():
    artifact = FrameSequenceArtifact(
        payload=b"zip-bytes",
        sha256=hashlib.sha256(b"zip-bytes").hexdigest(),
        frame_count=288,
        frame_duration_ms=42,
        frame_format="webp",
        overall_passed=False,
        failed_criteria=("1-尾巴完整", "3-帧间不闪烁"),
    )

    assert artifact.to_wire() == {
        "frameCount": 288,
        "frameDurationMs": 42,
        "frameFormat": "webp",
        "overallPassed": False,
        "failedCriteria": ["1-尾巴完整", "3-帧间不闪烁"],
    }


# ------------------------------------------------------------------ 端到端


def _synth_green_video(path: Path, *, size: int = 128, frames: int = 8, fps: int = 24) -> Path:
    """纯绿背景 + 移动色块。fps 取 24 是为了和 `build_frame_sequence` 的抽帧 fps 一致，
    否则 ffmpeg 会按 24fps 复制帧，帧数就不是 `frames` 了。

    色块取 (136,68,34)：`G - max(R,B) = -68`，远低于 `KEY_LOW`，必被判为前景。
    """
    sequence = path.parent / f"{path.stem}-seq"
    sequence.mkdir(parents=True, exist_ok=True)
    for index in range(frames):
        image = Image.new("RGB", (size, size), (0, 255, 0))
        left = 10 + index * 4
        ImageDraw.Draw(image).rectangle([left, 30, left + 40, 90], fill=(136, 68, 34))
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
def test_pack_frame_sequence_end_to_end_from_the_scratch_video(tmp_path: Path):
    state_dir = tmp_path / "state"
    video = motion_source_path(state_dir, "provider-1")
    video.parent.mkdir(parents=True)
    _synth_green_video(video)

    artifact = pack_frame_sequence(_request(), state_dir=state_dir, log=lambda _: None)

    assert artifact.frame_format == "webp"
    assert artifact.frame_count == 8, "24fps × 8 帧源，抽帧后仍该是 8 帧"
    assert artifact.frame_duration_ms == 42
    assert artifact.sha256 == hashlib.sha256(artifact.payload).hexdigest()

    with zipfile.ZipFile(io.BytesIO(artifact.payload)) as archive:
        names = archive.namelist()
        webp_frames = [name for name in names if name.endswith(".webp")]
        assert len(webp_frames) == artifact.frame_count
        assert "manifest.json" in names
        manifest = json.loads(archive.read("manifest.json").decode("utf-8"))
        # 验证 zip 自洽：manifest 里每个文件的 sha256 必须等于 zip 里的字节
        for entry in manifest["files"]:
            data = archive.read(entry["relativePath"])
            assert hashlib.sha256(data).hexdigest() == entry["sha256"]

    assert manifest["schemaVersion"] == 7
    assert manifest["renderer"] == "frame-sequence-v1"
    assert manifest["petId"] == "09-newcat"
    assert manifest["displayName"] == "我的猫"
    assert manifest["species"] == "cat"
    assert manifest["baseImage"].endswith(".webp")

    # 中间产物留档在 scratch（证据图**不在 zip 里** —— 那是刻意的：一个 job 一个 artifact）
    out_dir = video.parent / "pack-frame-sequence" / "attempt-1"
    assert (out_dir / "04-抠像" / "抠像参数.json").is_file()
    assert (out_dir / "05-验收" / "验收报告.json").is_file()
    assert not list((out_dir / "10-运行时包").rglob("*.png")), "包里不该混 PNG"


@pytest.mark.skipif(FFMPEG is None, reason="需要 ffmpeg 现场合成测试视频")
def test_retry_uses_a_fresh_attempt_directory(tmp_path: Path):
    """`build_frame_sequence` 只接受空目录（中间产物上百个文件，原地重跑要批量删除，
    会被本机守卫拦），所以每个 attempt 必须换一个新目录。"""
    state_dir = tmp_path / "state"
    video = motion_source_path(state_dir, "provider-1")
    video.parent.mkdir(parents=True)
    _synth_green_video(video, frames=4)

    first = pack_frame_sequence(_request(attempt=1), state_dir=state_dir, log=lambda _: None)
    second = pack_frame_sequence(_request(attempt=2), state_dir=state_dir, log=lambda _: None)

    assert first.sha256 != second.sha256 or first.payload == second.payload
    base = video.parent / "pack-frame-sequence"
    assert (base / "attempt-1").is_dir()
    assert (base / "attempt-2").is_dir()

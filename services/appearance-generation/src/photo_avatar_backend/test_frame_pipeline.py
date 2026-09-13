"""写实风两个 step 的服务侧装配测试。

端到端那条用 ffmpeg 现场合成一支 128×128 的假绿幕视频（**不烧算力**），
没有 ffmpeg 就 skip。合成夹具与 `frames/test_pipeline.py` 里那份是**有意重复**的：
测试夹具不跨模块借 —— 否则一边改了夹具，另一边的失败信息会变得莫名其妙。

`generateMotionSource` 全程用 `_FakeClient`：**一次 API 都不打**。
真实的那一步要花 5.69 算力，测试绝不能碰真钥匙。
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

from photo_avatar_backend.contracts import FrameStepRequest, SourceImage  # noqa: E402
from photo_avatar_backend.frame_pipeline import (  # noqa: E402
    FIRST_FRAME_MARGIN_LEFT,
    SCALE_LADDER,
    FramePipelineError,
    FrameSequenceArtifact,
    MotionSource,
    generate_motion_source,
    motion_source_path,
    pack_frame_sequence,
    scratch_dir,
)
from photo_avatar_backend.lk888_client import Lk888Error, MediaState  # noqa: E402

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


# ------------------------------------------------------------------ 假上游

MASTER_TASK = "task-master-1"
VIDEO_TASK = "task-video-1"


def _master_png(width_ratio: float = 0.82, *, size: int = 1024) -> bytes:
    """造一张「主体横向占 `width_ratio`」的透明母版。

    宽度是刻意可调的：`reposition_for_tail_swing` 的判断题是
    `左余量 + 主体宽×scale×(1 + 0.235) <= 1 - 0.05`，
    主体越宽越难通过 —— 0.82 恰好 0.90 挂、0.85 过；1.0 则整条阶梯都过不了。
    """
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    half = width_ratio / 2
    x0 = int(round((0.5 - half) * size))
    x1 = int(round((0.5 + half) * size)) - 1
    ImageDraw.Draw(image).rectangle([x0, 300, x1, 800], fill=(120, 90, 60, 255))
    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    return buffer.getvalue()


class _FakeClient:
    """只实现 `generate_motion_source` 用到的那五个方法。

    `poll_script` 让每个任务能指定「第几次轮询返回什么」，用来演失败与超时。
    """

    def __init__(
        self,
        *,
        master_png: bytes | None = None,
        video: bytes = b"fake-mp4-bytes",
        fail_with: Lk888Error | None = None,
        never_finishes: bool = False,
    ) -> None:
        self.master_png = master_png if master_png is not None else _master_png()
        self.video = video
        self.fail_with = fail_with
        self.never_finishes = never_finishes
        self.calls: list[str] = []
        self.prompts: dict[str, str] = {}
        self.image_batches: list[int] = []
        self.video_kwargs: dict[str, object] = {}

    def _state(self, task_id: str, url: str) -> MediaState:
        if self.fail_with is not None:
            return MediaState(task_id, "failed", True, None, self.fail_with)
        if self.never_finishes:
            return MediaState(task_id, "running", False, None, None)
        return MediaState(task_id, "success", True, url, None)

    def submit_image(self, prompt: str, images: object) -> str:
        self.calls.append("submit_image")
        self.prompts["master"] = prompt
        self.image_batches.append(len(images))  # type: ignore[arg-type]
        return MASTER_TASK

    def poll_image(self, task_id: str) -> MediaState:
        self.calls.append(f"poll_image:{task_id}")
        return self._state(task_id, f"https://example.test/{task_id}.png")

    def download(self, url: str) -> bytes:
        self.calls.append("download")
        return self.master_png

    def submit_video(self, prompt: str, *, images: object = (), **kwargs: object) -> str:
        self.calls.append("submit_video")
        self.prompts["loop"] = prompt
        self.image_batches.append(len(images))  # type: ignore[arg-type]
        self.video_kwargs = kwargs
        return VIDEO_TASK

    def download_video(self, url: str) -> bytes:
        self.calls.append("download_video")
        return self.video


# ------------------------------------------------------------------ generateMotionSource


def _motion_request(**overrides: object) -> FrameStepRequest:
    photo = SourceImage(source_id="photo-1", png=_master_png(), sha256="0" * 64,
                        width=1024, height=1024)
    payload: dict[str, object] = {
        "step": "generateMotionSource",
        "source_images": (photo,),
    }
    payload.update(overrides)
    return _request(**payload)


def test_generate_motion_source_walks_master_frame_video(tmp_path: Path):
    client = _FakeClient()

    result = generate_motion_source(
        _motion_request(), client=client, state_dir=tmp_path, log=lambda _: None
    )

    assert client.calls[:6] == [
        "submit_image",
        f"poll_image:{MASTER_TASK}",
        "download",
        "submit_video",
        f"poll_image:{VIDEO_TASK}",
        "download_video",
    ]
    # 母版吃 1 张照片，视频吃 1 张首帧
    assert client.image_batches == [1, 1]

    # 母版提示词：物种来自请求，毛长档位**不提**（请求里没有这个字段）
    assert "this exact cat." in client.prompts["master"]
    assert "short-haired" not in client.prompts["master"]
    assert client.prompts["loop"].startswith("以首帧图作为这只动物唯一的身份")

    # 视频规格是产品定死的，不是默认值
    assert client.video_kwargs == {
        "version": "标准",
        "duration": "12",
        "resolution": "480p",
        "aspect_ratio": "1:1",
        "mode": "shouweizhen",
    }

    assert result.reused is False
    assert result.video_task_id == VIDEO_TASK
    assert result.master_task_id == MASTER_TASK
    assert result.video_path == motion_source_path(tmp_path, "provider-1")
    assert result.video_path.read_bytes() == b"fake-mp4-bytes"
    assert result.video_bytes == len(b"fake-mp4-bytes")
    # 中间产物留档在 scratch，出问题时能整目录打包回看
    assert result.master_path is not None and result.master_path.is_file()
    assert result.first_frame_path is not None and result.first_frame_path.is_file()
    assert not list(tmp_path.rglob("*.part")), "原子写入不该留下 .part 残骸"


def test_generate_motion_source_converges_framing_on_the_free_ladder(tmp_path: Path):
    """0.82 宽的主体在 0.90 会甩尾出画、在 0.85 通过 —— 必须先收敛再进视频。"""
    client = _FakeClient(master_png=_master_png(0.82))

    result = generate_motion_source(
        _motion_request(), client=client, state_dir=tmp_path, log=lambda _: None
    )

    assert result.first_frame_scale == SCALE_LADDER[1] == 0.85
    assert result.first_frame_left_margin == FIRST_FRAME_MARGIN_LEFT
    assert result.first_frame_right_margin is not None
    assert result.first_frame_right_margin >= 0.05
    # 两次首帧尝试都发生在视频之前，且只提交了一次视频（钱只花一次）
    assert client.calls.count("submit_video") == 1
    ladder_dirs = sorted(
        path.name for path in (result.first_frame_path.parent.parent).iterdir()  # type: ignore[union-attr]
    )
    assert ladder_dirs == ["scale-85", "scale-90"]


def test_generate_motion_source_gives_up_before_paying_when_framing_never_fits(tmp_path: Path):
    """主体横向太长（长毛猫的尾巴常见）→ 报「换一张照片」，**不提交视频**。"""
    client = _FakeClient(master_png=_master_png(1.0))

    with pytest.raises(FramePipelineError) as failure:
        generate_motion_source(
            _motion_request(), client=client, state_dir=tmp_path, log=lambda _: None
        )

    assert failure.value.code == "invalidInput", "照片的问题，重试同一张只会再失败一次"
    assert "换一张" in str(failure.value)
    assert "submit_video" not in client.calls, "取景不合格就不许进视频（视频已经付过钱）"
    assert not motion_source_path(tmp_path, "provider-1").exists()


def test_generate_motion_source_reuses_an_existing_video_without_paying_again(tmp_path: Path):
    """后端重启会把成功的 job 结果清掉（`_load_state`）→ 同 session 重试要白捡回视频。"""
    video = motion_source_path(tmp_path, "provider-1")
    video.parent.mkdir(parents=True)
    video.write_bytes(b"already-paid-for")
    client = _FakeClient()

    result = generate_motion_source(
        _motion_request(), client=client, state_dir=tmp_path, log=lambda _: None
    )

    assert client.calls == [], "复用路径一次上游都不该打"
    assert result.reused is True
    assert result.video_bytes == len(b"already-paid-for")
    assert result.master_path is None and result.first_frame_path is None


def test_generate_motion_source_reports_exactly_one_provider_task(tmp_path: Path):
    """一个 job 的 `lk888_task_id` 是单选（上游删除要用它）—— 报第二个会被 store 拒掉。"""
    reported: list[str] = []

    generate_motion_source(
        _motion_request(),
        client=_FakeClient(),
        state_dir=tmp_path,
        report_task_id=reported.append,
        log=lambda _: None,
    )

    assert reported == [VIDEO_TASK], "只上报花钱的那一个"


def test_a_content_policy_rejection_is_not_swallowed(tmp_path: Path):
    """内容审核拒绝必须原样上抛：包成「服务暂时不可用」会让上层白重试一次。"""
    client = _FakeClient(fail_with=Lk888Error("contentPolicy", False, "输入文本可能包含敏感信息"))

    with pytest.raises(Lk888Error) as failure:
        generate_motion_source(
            _motion_request(), client=client, state_dir=tmp_path, log=lambda _: None
        )

    assert failure.value.code == "contentPolicy"
    assert failure.value.retryable is False


def test_a_hung_provider_surfaces_as_a_retryable_timeout(tmp_path: Path, monkeypatch):
    monkeypatch.setattr("photo_avatar_backend.frame_pipeline.MAX_WAIT_SECONDS", 0.0)
    monkeypatch.setattr("photo_avatar_backend.frame_pipeline.time.sleep", lambda _: None)
    client = _FakeClient(never_finishes=True)

    with pytest.raises(Lk888Error) as failure:
        generate_motion_source(
            _motion_request(), client=client, state_dir=tmp_path, log=lambda _: None
        )

    assert failure.value.code == "timeout"
    assert failure.value.retryable is True, "超时不等于失败：额度可能已经扣了，得让上层决定"


def test_generate_motion_source_refuses_another_step(tmp_path: Path):
    with pytest.raises(FramePipelineError, match="got step"):
        generate_motion_source(_request(), client=_FakeClient(), state_dir=tmp_path)


def test_generate_motion_source_refuses_a_request_without_photos(tmp_path: Path):
    with pytest.raises(FramePipelineError) as failure:
        generate_motion_source(
            _motion_request(source_images=()),
            client=_FakeClient(),
            state_dir=tmp_path,
        )
    assert failure.value.code == "invalidInput"


def test_generate_motion_source_refuses_a_missing_provider_session(tmp_path: Path):
    with pytest.raises(FramePipelineError, match="requires a providerSessionId"):
        generate_motion_source(
            _motion_request(provider_session_id=None),
            client=_FakeClient(),
            state_dir=tmp_path,
        )


def test_motion_source_wire_is_json_safe():
    """状态要落盘成 JSON —— Path 之类的类型漏进去会让整条 job 写不出去。"""
    source = MotionSource(
        out_dir=Path("out"),
        video_path=Path("out/motion-source.mp4"),
        master_path=Path("out/母版.png"),
        first_frame_path=Path("out/绿幕首帧-1024.png"),
        master_task_id=MASTER_TASK,
        video_task_id=VIDEO_TASK,
        first_frame_scale=0.85,
        first_frame_left_margin=0.06,
        first_frame_right_margin=0.0792,
        video_bytes=123,
        reused=False,
    )

    wire = source.to_wire()
    assert json.loads(json.dumps(wire)) == wire
    assert not any(isinstance(value, Path) for value in wire.values())


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

"""写实风两个 step 的服务侧装配测试。

端到端那条用 ffmpeg 现场合成一支 128×128 的假绿幕视频（**不烧算力**），
没有 ffmpeg 就 skip。合成夹具与 `frames/test_pipeline.py` 里那份是**有意重复**的：
测试夹具不跨模块借 —— 否则一边改了夹具，另一边的失败信息会变得莫名其妙。

`generateMotionSource` 全程用 `_FakeClient`：**一次 API 都不打**。
真实的那一步要花 5.02 算力，测试绝不能碰真钥匙。
"""

from __future__ import annotations

import hashlib
import io
import json
import shutil
import subprocess
import sys
import zipfile
from collections.abc import Mapping
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
    analyze_action_facts,
    analyze_photo_facts,
    generate_motion_source,
    motion_source_path,
    pack_frame_sequence,
    scratch_dir,
    upload_variant_id,
)
from photo_avatar_backend.frames.action_prompts import (  # noqa: E402
    ACTION_IDS,
    FALLBACK_ACTION_FACTS,
    render_action_prompt_for,
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

    同一个 providerSession 重试要命中同一个文件，才谈得上「复用不重付 5.02 算力」。
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
    """只实现 `generate_motion_source` 用到的那六个方法。

    失败/超时靠 `fail_with` 与 `never_finishes` 演：任务一转终态就带错误，或者永远不转终态。
    """

    def __init__(
        self,
        *,
        master_png: bytes | None = None,
        video: bytes = b"fake-mp4-bytes",
        fail_with: Lk888Error | None = None,
        never_finishes: bool = False,
        facts: Mapping[str, object] | Exception | None = None,
    ) -> None:
        self.master_png = master_png if master_png is not None else _master_png()
        self.video = video
        self.fail_with = fail_with
        self.never_finishes = never_finishes
        # 照片分析的回包：默认「看不出毛长」，要测毛长档位时显式传。
        self.facts = {"species": "cat", "coat": "unknown"} if facts is None else facts
        self.calls: list[str] = []
        self.prompts: dict[str, str] = {}
        self.image_batches: list[int] = []
        self.analysis_image_counts: list[int] = []
        self.video_kwargs: dict[str, object] = {}

    def _state(self, task_id: str, url: str) -> MediaState:
        if self.fail_with is not None:
            return MediaState(task_id, "failed", True, None, self.fail_with)
        if self.never_finishes:
            return MediaState(task_id, "running", False, None, None)
        return MediaState(task_id, "success", True, url, None)

    def analyze_json(self, prompt: str, images: object, schema: object) -> object:
        self.calls.append("analyze_json")
        self.analysis_image_counts.append(len(images))  # type: ignore[arg-type]
        if isinstance(self.facts, Exception):
            raise self.facts
        return self.facts

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


def test_generate_motion_source_walks_photos_facts_master_frame_video(tmp_path: Path):
    client = _FakeClient()

    result = generate_motion_source(
        _motion_request(), client=client, state_dir=tmp_path, log=lambda _: None
    )

    assert client.calls == [
        # 看照片在母版**之前**：毛长要赶得上进母版提示词
        "analyze_json",
        "submit_image",
        f"poll_image:{MASTER_TASK}",
        "download",
        "submit_video",
        f"poll_image:{VIDEO_TASK}",
        "download_video",
    ]
    # 分析看的是原始照片；母版吃 1 张照片，视频吃 1 张首帧
    assert client.analysis_image_counts == [1]
    assert client.image_batches == [1, 1]

    # 母版提示词：物种来自请求；分析说「看不出毛长」→ 不提档位
    assert "this exact cat." in client.prompts["master"]
    assert "short-haired" not in client.prompts["master"]
    assert "long-haired" not in client.prompts["master"]
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


# ------------------------------------------------------------------ 看照片


def test_photo_analysis_puts_the_coat_hint_into_the_master_prompt(tmp_path: Path):
    """长毛猫值钱的就是这一句：没有它，母版容易把围脖和尾巴画短。"""
    client = _FakeClient(facts={"species": "cat", "coat": "long"})

    generate_motion_source(_motion_request(), client=client, state_dir=tmp_path, log=lambda _: None)

    assert "this exact long-haired cat." in client.prompts["master"]
    assert "short-haired" not in client.prompts["master"]


def test_a_failed_photo_analysis_only_costs_the_hint(tmp_path: Path):
    """分析是**可选**步骤：上游挂了也不许弄死一次要花 5.02 算力的生成。"""
    client = _FakeClient(facts=Lk888Error("temporaryUnavailable", True, "analyze boom"))
    lines: list[str] = []

    result = generate_motion_source(
        _motion_request(), client=client, state_dir=tmp_path, log=lines.append
    )

    assert result.video_task_id == VIDEO_TASK, "照常出母版、照常出视频"
    assert "short-haired" not in client.prompts["master"]
    assert "long-haired" not in client.prompts["master"]
    assert any("跳过毛长档位" in line for line in lines), "降级必须留痕，否则没人知道它被骗过"


def test_a_content_policy_rejection_of_the_analysis_is_also_only_a_hint(tmp_path: Path):
    """审核拒绝照片时也是降级 —— 真被拒的话母版那一步会自己再报一次，不用这里抢先。"""
    client = _FakeClient(facts=Lk888Error("contentPolicy", False, "输入图片可能包含敏感信息"))

    result = generate_motion_source(
        _motion_request(), client=client, state_dir=tmp_path, log=lambda _: None
    )

    assert result.reused is False
    assert "submit_video" in client.calls


@pytest.mark.parametrize(
    "facts",
    [
        {"species": "cat", "coat": "unknown"},  # 照片里看不出来
        {"species": "cat", "coat": "medium"},  # 模型没按 schema 回
        {"species": "cat"},  # 少了字段
        {"species": "cat", "coat": None},
        "not-an-object",  # 回包不成形
    ],
)
def test_an_unusable_analysis_answer_degrades_to_no_hint(tmp_path: Path, facts: object):
    """看不出来 / 答非所问 / 回包不成形 —— 都得变成「不提」，不能塞进提示词。"""
    client = _FakeClient(facts=facts)

    generate_motion_source(_motion_request(), client=client, state_dir=tmp_path, log=lambda _: None)

    assert "short-haired" not in client.prompts["master"]
    assert "long-haired" not in client.prompts["master"]


def test_a_species_mismatch_is_logged_but_changes_nothing(tmp_path: Path):
    """物种是产品写进 manifest 的字段，不该被一次模型判断推翻 —— 但值得留痕。"""
    client = _FakeClient(facts={"species": "dog", "coat": "short"})
    lines: list[str] = []

    generate_motion_source(_motion_request(), client=client, state_dir=tmp_path, log=lines.append)

    assert "this exact short-haired cat." in client.prompts["master"], "物种仍以请求为准"
    assert any("照片看着像 dog" in line for line in lines)


def test_a_species_mismatch_is_not_an_error(tmp_path: Path):
    """`analyze_photo_facts` 只回事实，不抛错 —— 判错物种不该让照片分析变成「失败」。"""
    facts = analyze_photo_facts(
        _motion_request(),
        client=_FakeClient(facts={"species": "dog", "coat": "long"}),
        log=lambda _: None,
    )

    assert (facts.species, facts.coat) == ("dog", "long")


def test_photo_analysis_reports_every_photo_it_was_given(tmp_path: Path):
    """判毛长要看全给的每一张 —— 只喂第一张容易把背面照当成正面。"""
    photo = SourceImage(source_id="photo-1", png=_master_png(), sha256="0" * 64,
                        width=1024, height=1024)
    client = _FakeClient(facts={"species": "cat", "coat": "short"})

    analyze_photo_facts(
        _motion_request(source_images=(photo, photo, photo)), client=client, log=lambda _: None
    )

    assert client.analysis_image_counts == [3]


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


def test_the_packed_variant_id_is_the_one_finalization_expects(tmp_path: Path):
    """安装包时 Rust 侧 finalization 拿 manifest 的 `variantId` 与它自己拼的
    `photo-avatar-<sessionId>-<revision>` 比对 —— 用 `combo-loop-v1`（内置宠物那个
    默认值）会在**用户点「接受并安装」**时才炸，前面两步全绿，极难定位。

    这条就是钉住「不要沿用默认值」：真跑一次要 5.02 算力，测试里绝不重跑，
    只断言打包写进 manifest 的那个值。
    """
    state_dir = tmp_path / "state"
    video = motion_source_path(state_dir, "provider-1")
    video.parent.mkdir(parents=True)
    _synth_green_video(video, frames=4)

    artifact = pack_frame_sequence(
        _request(session_id="session-abc", revision=3),
        state_dir=state_dir,
        log=lambda _: None,
    )
    manifest = json.loads(
        (video.parent / "pack-frame-sequence" / "attempt-1" / "10-运行时包" / "manifest.json").read_text(
            encoding="utf-8"
        )
    )
    assert manifest["variantId"] == "photo-avatar-session-abc-3"
    assert manifest["variantId"] == upload_variant_id("session-abc", 3)


# ---------------- 动作提示词的 4 个宠物字段（gpt-4o 看绿幕首帧图） ----------------
#
# 服务路径没有 `output/宠物档案/<petId>.json`（那份在 .gitignore 里、而且只有 04/05/06），
# 所以这 4 项改由模型从**首帧图**判。纪律与毛长那条一致：**只能降级，不能失败**。

_ACTION_FACTS_OK = {
    "identity": "短毛猫，圆脸、大而圆的眼睛、三角形耳朵",
    "coat": "浅奶油白底毛（亮部 RGB≈(236,228,220)）+ 深色近黑斑纹",
    "coat_guard": "不得变灰、不得变黄、不得整体提亮",
    "coat_negative": "desaturation, grey, yellowing",
}


def test_action_facts_are_taken_from_the_frame_when_the_model_answers() -> None:
    client = _FakeClient(facts=_ACTION_FACTS_OK)

    facts = analyze_action_facts(client, b"green-screen-frame-png", log=lambda _: None)

    assert facts.detected
    assert facts.identity == _ACTION_FACTS_OK["identity"]
    assert facts.coat == _ACTION_FACTS_OK["coat"]
    assert facts.coat_guard == _ACTION_FACTS_OK["coat_guard"]
    assert facts.coat_negative == _ACTION_FACTS_OK["coat_negative"]
    # 判的是**首帧图那一张**，不是原照片那批（首帧图才是整段视频的锚）。
    assert client.analysis_image_counts == [1]


@pytest.mark.parametrize(
    "response",
    [
        # 答了哨兵值
        {"identity": "unknown", "coat": "c", "coat_guard": "g", "coat_negative": "n"},
        # 某个字段是空串（答非所问 / 模型偷懒）
        {"identity": "猫", "coat": "   ", "coat_guard": "g", "coat_negative": "n"},
        # 缺字段
        {"identity": "猫", "coat": "c", "coat_guard": "g"},
        # 回包根本不是对象
        ["not", "an", "object"],
    ],
)
def test_an_unusable_response_falls_back_as_a_whole(response: object) -> None:
    """四种「不可用」抹平成同一个结果，而且是**整包**换兜底 ——
    兜底本身是自洽的一套措辞，混用「真判的 identity + 兜底的 coat」只会自相矛盾。"""
    client = _FakeClient(facts=response)

    facts = analyze_action_facts(client, b"png", log=lambda _: None)

    assert facts == FALLBACK_ACTION_FACTS
    assert not facts.detected


def test_an_upstream_error_degrades_instead_of_failing() -> None:
    """一次失败会带走后面 ~5 算力/支的视频，而少这四项母版/视频照样出得来
    （真正的身份锚是首帧图本身）—— 纯提质项不该有这个权力。"""
    client = _FakeClient(facts=Lk888Error("temporaryUnavailable", True, "analyze boom"))

    facts = analyze_action_facts(client, b"png", log=lambda _: None)

    assert facts == FALLBACK_ACTION_FACTS


def test_a_code_error_is_not_swallowed() -> None:
    """`AttributeError` 之类的 bug 必须炸出来 —— 吞掉它就永远查不到。"""

    class _Broken:
        def analyze_json(self, prompt: str, images: object, schema: object) -> object:
            raise AttributeError("analyze_json 拼错了")

    with pytest.raises(AttributeError):
        analyze_action_facts(_Broken(), b"png", log=lambda _: None)


@pytest.mark.parametrize("action_id", sorted(ACTION_IDS))
def test_the_fallback_still_renders_a_valid_prompt(action_id: str) -> None:
    """兜底也得能真的渲染出提示词 —— 否则「降级」等于「这条动作做不出来」。"""
    text = render_action_prompt_for(action_id, FALLBACK_ACTION_FACTS)

    assert FALLBACK_ACTION_FACTS.identity in text
    assert FALLBACK_ACTION_FACTS.coat_guard in text
    assert FALLBACK_ACTION_FACTS.coat_negative in text
    for token in ("__IDENTITY__", "__COAT__", "__ACTION_", "__TAIL_"):
        assert token not in text

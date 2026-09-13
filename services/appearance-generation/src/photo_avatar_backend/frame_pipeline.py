# -*- coding: utf-8 -*-
"""写实风（`frame-video-v1`）两个 step 的服务侧实现。

    generateMotionSource   照片 → 透明母版 → 绿幕首帧 → Seedance 绿幕视频   （**要花钱**）
    packFrameSequence      scratch 里的 mp4 → 抠像 → 验收 → WebP → zip      （**0 算力**）

两个 step 按「钱」切：视频失败要重付约 5.69 算力，粒度不能太粗；而后面半段
（抠像/验收/打包）便宜到可以整段重跑。

## 产物的边界（这几条是设计决策，不是实现细节）

- 中间的 mp4 落在 `state_dir/scratch/<providerSessionId>/motion-source.mp4`，
  **不上传、也不经客户端**。同一个 providerSession 重试能直接复用视频（不重付算力），
  只有显式「重新生成」才要求重跑。这样「白做」的代价从一次视频降到 0。
- 交付物只有**一支 zip**（打包好的 schema 7 运行时包），走现有 artifact 端点。
  验收证据图**留在服务侧**：客户端要人工确认的是「桌宠动起来像不像」（第 6 片装进预览位），
  不是那五张证据图；`overall_passed` / `failed_criteria` 随 job 结果回去，
  够客户端把「检测到哪条异常」提示给用户。
  一个 job 只能有一个 artifact，这是现有契约的硬约束。
- 中间目录按 `attempt` 分子目录：`frames.pipeline.build_frame_sequence` 只接受**空目录**
  （中间产物上百个文件，原地重跑要批量删除，会被本机的批量删除守卫拦）。
  代价是重试会留档多份中间产物 —— 那些在 `state_dir/` 下，后续片再加清理。

## 提示词住在哪

`generateMotionSource` 的两份提示词（母版 + 组合循环）在
`assets/motion-prompts/`，由 `frames.prompts` 渲染。**正文没变**：
是从原来那两份 `output/_通用*.txt`（**被 .gitignore 忽略**，服务根本拿不到）
里用老脚本自己的切分逻辑抽出来的，抽完逐字节比对过。见落地清单第 5b 片。
"""
from __future__ import annotations

import hashlib
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .contracts import FrameStepRequest
from .lk888_client import Lk888Error

SCRATCH_DIR = "scratch"
MOTION_SOURCE_FILE = "motion-source.mp4"
PACK_SUBDIR = "pack-frame-sequence"
# 母版/首帧的中间产物与 mp4 同级，方便出问题时整目录打包回看
MOTION_SUBDIR = "motion-source"
MASTER_DIR = "00-母版"
FIRST_FRAME_DIR = "01-首帧"

# 产品固定 WebP（与内置宠物 04/05 的现有资产一致）。
FRAME_FORMAT = "webp"

# ---------------- 写实风的成套规格（改这些等于改产品，不是调参） ----------------
#
# ⚠️ 别拿 Mini 档（0.1728 算力/秒）估标准档（0.474 算力/秒）—— 同样 12s 差约 3 倍。
# 实测：seedance + 标准 + 480p + 12s = 5.69 算力/支；母版（gpt-image-2）只要 0.06。
# 时长 12s / 24fps / 42ms 与内置资产同规格，换数值会让新宠物的节奏与内置不一致。
VIDEO_VERSION = "标准"
VIDEO_DURATION = "12"
VIDEO_RESOLUTION = "480p"
VIDEO_ASPECT_RATIO = "1:1"
VIDEO_MODE = "shouweizhen"

# 首帧取景收敛的免费阶梯：首帧不烧算力，所以可以从宽到紧试。
# 两个余量（左余量、尾巴甩到最远时的右余量）都要 ≥ MIN_FRAMING_MARGIN。
SCALE_LADDER = (0.90, 0.85, 0.80, 0.75)
# 别低于 0.05：0.02 会让主体贴左边触边。
FIRST_FRAME_MARGIN_LEFT = 0.06

# 视频是分钟级的任务；轮询间隔与超时对齐老脚本 `poc_生成绿幕视频.py` 的默认值。
POLL_INTERVAL_SECONDS = 10.0
MAX_WAIT_SECONDS = 900.0


class FramePipelineError(ValueError):
    """写实风 step 的失败态。

    刻意继承 `ValueError` 而不是抛 `SystemExit`：**`SystemExit` 不是 `Exception`**，
    `job_store.run_reserved` 的 `except Exception` 抓不到它 —— 会在 worker 线程里
    静默逃逸，表现为「job 永远 running」、日志一片空白。

    `code` 决定客户端看到的错误码：
    - `temporaryUnavailable`（默认）：服务侧的毛病，重试有意义；
    - `invalidInput`：**照片本身的问题**（取景收敛到最紧的 scale 仍出画）——
      重试同一张照片只会再失败一次，要提示用户换一张。
    上游的 `Lk888Error`（内容审核 / 额度 / 网络…）**原样往上抛**，不要在这里包一层：
    包了就丢掉了「这个错误能不能重试」这个唯一重要的信息。
    """

    def __init__(self, message: str, *, code: str = "temporaryUnavailable") -> None:
        super().__init__(message)
        self.code = code


@dataclass(frozen=True)
class MotionSource:
    """`generateMotionSource` 的产物。

    **没有 artifact** —— 这一步的产物是一支留在服务侧的 mp4（`video_path`），
    不往客户端送任何字节。客户端要的只是「成了没有」+「花了哪一次」。

    `master_path` / `first_frame_path` 在 `reused` 时为 `None`：复用已有 mp4 时
    不会重跑母版与首帧（那才是「不重付」的意义）。
    """

    out_dir: Path
    video_path: Path
    master_path: Path | None
    first_frame_path: Path | None
    master_task_id: str | None
    video_task_id: str | None
    first_frame_scale: float | None
    first_frame_left_margin: float | None
    first_frame_right_margin: float | None
    video_bytes: int
    reused: bool

    def to_wire(self) -> dict[str, object]:
        """进 job 状态的元数据。client 靠它显示「要不要人工确认」之前的进度。"""
        return {
            "videoBytes": self.video_bytes,
            "reused": self.reused,
            "masterTaskId": self.master_task_id,
            "videoTaskId": self.video_task_id,
            "firstFrameScale": self.first_frame_scale,
            "firstFrameLeftMargin": self.first_frame_left_margin,
            "firstFrameRightMargin": self.first_frame_right_margin,
        }


@dataclass(frozen=True)
class FrameSequenceArtifact:
    """一个 job 的交付物：一支打包好的 schema 7 运行时包（zip 字节）。"""

    payload: bytes
    sha256: str
    frame_count: int
    frame_duration_ms: int
    frame_format: str
    overall_passed: bool
    failed_criteria: tuple[str, ...]

    @classmethod
    def from_build(cls, build: object) -> "FrameSequenceArtifact":
        payload = build.zip_path.read_bytes()  # type: ignore[attr-defined]
        return cls(
            payload=payload,
            sha256=hashlib.sha256(payload).hexdigest(),
            frame_count=build.frame_count,  # type: ignore[attr-defined]
            frame_duration_ms=build.frame_duration_ms,  # type: ignore[attr-defined]
            frame_format=build.frame_format,  # type: ignore[attr-defined]
            overall_passed=build.overall_passed,  # type: ignore[attr-defined]
            failed_criteria=tuple(build.failed_criteria),  # type: ignore[attr-defined]
        )

    def to_wire(self) -> dict[str, object]:
        """进 job 状态的**元数据**（不含 zip 字节）。

        `job_store` 会把 zip 单独写进 `artifacts/<id>.zip`；这里这几项是给 `_job_wire`
        回给客户端用的 —— 客户端靠 `overallPassed` / `failedCriteria` 决定要不要在
        「人工确认」那一步提示用户「检测到异常」，而不是靠证据图（证据图留服务侧）。
        """
        return {
            "frameCount": self.frame_count,
            "frameDurationMs": self.frame_duration_ms,
            "frameFormat": self.frame_format,
            "overallPassed": self.overall_passed,
            "failedCriteria": list(self.failed_criteria),
        }


def scratch_root(state_dir: Path) -> Path:
    return Path(state_dir) / SCRATCH_DIR


def scratch_dir(state_dir: Path, provider_session_id: str) -> Path:
    return scratch_root(state_dir) / provider_session_id


def motion_source_path(state_dir: Path, provider_session_id: str) -> Path:
    """中间 mp4 的位置。**key 是 providerSessionId，不是 jobId** ——
    同一个 providerSession 重试要命中同一个文件，才谈得上「复用不重付」。"""
    return scratch_dir(state_dir, provider_session_id) / MOTION_SOURCE_FILE


def generate_motion_source(
    request: FrameStepRequest,
    *,
    client: Any,
    state_dir: Path,
    report_task_id: Callable[[str], None] | None = None,
    log: Callable[[str], None] = print,
) -> MotionSource:
    """`generateMotionSource`：照片 → 母版 → 绿幕首帧 → 绿幕视频。

    **唯一花钱的 step**（≈5.75 算力/次）。三条防线保证不白花：

    1. **已有 mp4 就直接复用**（不重跑、不重付）。后端重启会清掉 job 结果
       （`_load_state` 把非 `renderTextureAtlas` 的成功态标 failed），这时候客户端
       用同一个 `providerSessionId` 重试，就能白捡回那支视频。
       要真重跑就换一个 `providerSessionId`（= 用户点「重新生成」）。
    2. **取景收敛在免费阶梯上做完再进视频**：首帧不烧算力，两个余量不达标就调小
       `scale` 重出，全挂才报错 —— 长毛猫当初没做这一步，白花 4.67 算力。
    3. **上游错误码原样透出**：`contentPolicy` 这类不可重试的错误必须让上层看见，
       否则会自动重试同一份提示词、必然再被拒一次。
    """
    if request.step != "generateMotionSource":
        raise FramePipelineError(f"generate_motion_source got step {request.step!r}")
    if not request.source_images:
        # 契约层已拦过；这里再挡一次是因为「没有照片」会变成一句空泛的 provider 报错。
        raise FramePipelineError(
            "generateMotionSource requires at least one photo", code="invalidInput"
        )
    provider_session_id = request.provider_session_id
    if provider_session_id is None:
        # `providerSessionId` 是 scratch 的 key：没有它，mp4 会落到一个没有主人的目录，
        # 「重试复用」直接失效。
        raise FramePipelineError("generateMotionSource requires a providerSessionId")

    video_path = motion_source_path(state_dir, provider_session_id)
    if video_path.is_file() and video_path.stat().st_size > 0:
        log(f"[复用] scratch 里已有 {MOTION_SOURCE_FILE}，跳过母版/首帧/视频（不重付算力）")
        return _reused_motion_source(video_path)

    work_dir = scratch_dir(state_dir, provider_session_id) / MOTION_SUBDIR / (
        f"attempt-{request.attempt}"
    )

    log("[1/3] 生成透明母版（gpt-image-2，约 0.06 算力）")
    master_path, master_task_id = _generate_master(
        client=client, request=request, work_dir=work_dir, log=log
    )

    log("[2/3] 绿幕首帧 + 取景收敛（免费阶梯，不进视频）")
    fitted = _converge_first_frame(master_path, work_dir=work_dir, state_dir=state_dir, log=log)

    log(f"[3/3] 生成绿幕视频（{VIDEO_VERSION} / {VIDEO_RESOLUTION} / {VIDEO_DURATION}s，约 5.69 算力）")
    video_task_id = _generate_video(
        client=client,
        first_frame=fitted.frame_png,
        video_path=video_path,
        report_task_id=report_task_id,
        log=log,
    )

    return MotionSource(
        out_dir=work_dir,
        video_path=video_path,
        master_path=master_path,
        first_frame_path=fitted.frame_png,
        master_task_id=master_task_id,
        video_task_id=video_task_id,
        first_frame_scale=fitted.scale,
        first_frame_left_margin=fitted.left_margin,
        first_frame_right_margin=fitted.right_margin_at_full_swing,
        video_bytes=video_path.stat().st_size,
        reused=False,
    )


def pack_frame_sequence(
    request: FrameStepRequest,
    *,
    state_dir: Path,
    log: Callable[[str], None] = print,
) -> FrameSequenceArtifact:
    """`packFrameSequence`：吃 scratch 里的 mp4，出一个 schema 7 运行时包。"""
    if request.step != "packFrameSequence":
        raise FramePipelineError(f"pack_frame_sequence got step {request.step!r}")
    provider_session_id = request.provider_session_id
    if provider_session_id is None:
        # 契约层已经拦过；这里再挡一次是因为「没有 providerSessionId 就找不到 mp4」，
        # 拼出一个空目录名会变成更难查的错。
        raise FramePipelineError("packFrameSequence requires a providerSessionId")

    video = motion_source_path(state_dir, provider_session_id)
    if not video.is_file():
        raise FramePipelineError(
            f"motion source video is missing: {video}"
            "（generateMotionSource 没跑成功，或后端重启后 scratch 被清掉了）"
        )

    # 懒 import：`frames.pipeline` 会拉起 numpy/PIL/scipy。放在模块顶层的话，
    # 后端启动就会硬依赖这些包 —— 缺一个就整个后端起不来，而不是只有这一步失败。
    from .frames.pipeline import build_frame_sequence

    out_dir = scratch_dir(state_dir, provider_session_id) / PACK_SUBDIR / f"attempt-{request.attempt}"
    log(f"[packFrameSequence] mp4={video.name}  输出={out_dir}")
    build = build_frame_sequence(
        video,
        out_dir,
        pet_id=request.pet_id,
        display_name=request.display_name,
        species=request.species,
        path_base=state_dir,
        log=log,
    )
    return FrameSequenceArtifact.from_build(build)


# ------------------------------------------------------------------ 内部实现


@dataclass(frozen=True)
class _FittedFirstFrame:
    frame_png: Path
    scale: float
    left_margin: float
    right_margin_at_full_swing: float


def _reused_motion_source(video_path: Path) -> MotionSource:
    return MotionSource(
        out_dir=video_path.parent,
        video_path=video_path,
        master_path=None,
        first_frame_path=None,
        master_task_id=None,
        video_task_id=None,
        first_frame_scale=None,
        first_frame_left_margin=None,
        first_frame_right_margin=None,
        video_bytes=video_path.stat().st_size,
        reused=True,
    )


def _wait_for_media(client: Any, task_id: str, *, label: str) -> Any:
    """轮询到终态。

    `state.error` 是 provider 给出的 `Lk888Error`（带 code / retryable）—— **原样上抛**，
    它是「该不该重试」的唯一依据。走到超时不等于失败：额度可能已经扣了，
    所以超时用 `timeout`（可重试），让上层决定。
    """
    deadline = time.monotonic() + MAX_WAIT_SECONDS
    last_state: str | None = None
    while True:
        state = client.poll_image(task_id)
        if state.error is not None:
            raise state.error
        if state.is_final:
            break
        if state.state != last_state:
            last_state = state.state
        if time.monotonic() >= deadline:
            raise Lk888Error("timeout", True, f"{label} timed out after {MAX_WAIT_SECONDS:.0f}s")
        time.sleep(POLL_INTERVAL_SECONDS)
    if state.state != "success" or not state.result_url:
        raise Lk888Error("temporaryUnavailable", True, f"{label} did not succeed")
    return state


def _generate_master(
    *,
    client: Any,
    request: FrameStepRequest,
    work_dir: Path,
    log: Callable[[str], None],
) -> tuple[Path, str]:
    """照片 → 透明母版。

    ⚠️ **不带毛长档位**：通用母版提示词里那个 `{{COAT_LEN}}` 位置是「short-haired /
    long-haired」二选一的**提示**，而 `FrameStepRequest` 里没有毛长字段（产品里也
    问不出来）。所以走不带提示的那条渲染 —— 提示词下一段本来就要求「毛长照照片一模一样」，
    真实来源是照片不是这个词。脚本 `一键出宠.py` 仍然可以显式传（人工出宠时知道）。
    """
    from .frames.prompts import render_master_prompt

    prompt = render_master_prompt(request.species)
    task_id = client.submit_image(prompt, [image.png for image in request.source_images])
    state = _wait_for_media(client, task_id, label="母版")
    png = client.download(state.result_url)

    master_path = work_dir / MASTER_DIR / f"母版-{task_id}.png"
    master_path.parent.mkdir(parents=True, exist_ok=True)
    master_path.write_bytes(png)
    log(f"[母版] task={task_id}  {master_path.name}  {len(png) // 1024} KB")
    return master_path, task_id


def _converge_first_frame(
    master_path: Path,
    *,
    work_dir: Path,
    state_dir: Path,
    log: Callable[[str], None],
) -> _FittedFirstFrame:
    """免费阶梯：从宽到紧试 `scale`，两个余量都够才放行。

    取景不合格**不许进视频** —— 尾巴会出画，而且视频已经付过钱了。
    """
    from .frames.first_frame import (
        MIN_FRAMING_MARGIN,
        FirstFrameError,
        compose_green_first_frame,
    )

    attempted: list[tuple[float, float, float]] = []
    last_error: str | None = None
    for scale in SCALE_LADDER:
        out_dir = work_dir / FIRST_FRAME_DIR / f"scale-{int(round(scale * 100))}"
        try:
            result = compose_green_first_frame(
                [("master", master_path)],
                out_dir,
                scale=scale,
                margin_left=FIRST_FRAME_MARGIN_LEFT,
                path_base=state_dir,
                log=log,
            )
        except FirstFrameError as exc:
            # 母版本身不可用（例如全是透明像素）—— 那是照片/母版的问题，不是服务故障。
            raise FramePipelineError(str(exc), code="invalidInput") from exc

        left = result.left_margin
        right = result.right_margin_at_full_swing
        if left is None or right is None:
            raise FramePipelineError(
                f"首帧没有取景余量可判（scale={scale}）", code="invalidInput"
            )
        log(
            f"[取景] scale={scale:.2f}  左余量={left * 100:.1f}%  "
            f"尾巴全摆时右余量={right * 100:.1f}%  （下限 {MIN_FRAMING_MARGIN * 100:.0f}%）"
        )
        if result.framing_ok:
            return _FittedFirstFrame(result.frame_png, scale, left, right)
        attempted.append((scale, left, right))
        last_error = (
            f"scale={scale:.2f} 左 {left * 100:.1f}% / 右 {right * 100:.1f}%"
        )

    ladder = "，".join(f"{scale:.2f}" for scale in SCALE_LADDER)
    raise FramePipelineError(
        f"取景收敛失败：免费阶梯（{ladder}）试完仍有两个余量之一不足"
        f"（最后 {last_error}；下限 {MIN_FRAMING_MARGIN * 100:.0f}%）"
        "。主体横向太长（长毛猫的尾巴常见），这张照片不适合生成桌宠，"
        "请换一张正面坐姿、尾巴收拢的照片。",
        code="invalidInput",
    )


def _generate_video(
    *,
    client: Any,
    first_frame: Path,
    video_path: Path,
    report_task_id: Callable[[str], None] | None,
    log: Callable[[str], None],
) -> str:
    """首帧 + 组合循环提示词 → 绿幕视频，落盘到约定的 mp4 路径。

    **只上报视频这一个 task id。** 一艘 job 的 `lk888_task_id` 是单选（上游删除要用
    它），报第二个不同 id 会被 `job_store` 拒掉并让整个 step 失败。母版那一个记在
    `MotionSource.master_task_id` 里回给客户端，追溯够用。
    """
    from .frames.prompts import render_loop_prompt

    task_id = client.submit_video(
        render_loop_prompt(),
        images=[first_frame.read_bytes()],
        version=VIDEO_VERSION,
        duration=VIDEO_DURATION,
        resolution=VIDEO_RESOLUTION,
        aspect_ratio=VIDEO_ASPECT_RATIO,
        mode=VIDEO_MODE,
    )
    if report_task_id is not None:
        report_task_id(task_id)
    log(f"[视频] task={task_id}，开始轮询（最多 {MAX_WAIT_SECONDS:.0f}s）")

    state = _wait_for_media(client, task_id, label="视频")
    payload = client.download_video(state.result_url)

    # 先写 .part 再原子改名：中途崩了不会留下一支「看起来存在、其实截断」的 mp4 ——
    # `packFrameSequence` 只判断 video.is_file()，半支视频会被当成完整的往下跑。
    video_path.parent.mkdir(parents=True, exist_ok=True)
    staging = video_path.with_name(video_path.name + ".part")
    staging.write_bytes(payload)
    staging.replace(video_path)
    log(f"[视频] {video_path.name}  {len(payload) // 1024} KB")
    return task_id

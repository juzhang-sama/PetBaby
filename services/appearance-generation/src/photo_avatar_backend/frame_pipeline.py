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
"""
from __future__ import annotations

import hashlib
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from .contracts import FrameStepRequest

SCRATCH_DIR = "scratch"
MOTION_SOURCE_FILE = "motion-source.mp4"
PACK_SUBDIR = "pack-frame-sequence"

# 产品固定 WebP（与内置宠物 04/05 的现有资产一致）。
FRAME_FORMAT = "webp"


class FramePipelineError(ValueError):
    """写实风 step 的失败态。

    刻意继承 `ValueError` 而不是抛 `SystemExit`：**`SystemExit` 不是 `Exception`**，
    `job_store.run_reserved` 的 `except Exception` 抓不到它 —— 会在 worker 线程里
    静默逃逸，表现为「job 永远 running」、日志一片空白。
    """


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


def generate_motion_source(
    request: FrameStepRequest,
    *,
    state_dir: Path,
    log: Callable[[str], None] = print,
) -> Path:
    """`generateMotionSource`：照片 → 母版 → 绿幕首帧 → 绿幕视频。

    **尚未实现**，见 `docs/设计/写实风产品化落地-2026-09-13.md` 第 5b 片。
    """
    raise FramePipelineError(
        "generateMotionSource is not implemented yet: "
        "母版/视频的提示词现在还是 scripts/_提示词模板/骨架.txt 那种"
        "«给人看的完整文档»（含「背景」「为什么」章节，靠 section() 切出主/负向提示词），"
        "服务化前要先做「提示词契约化」把「给人读的」和「给机器用的」拆开。"
        "见落地清单第 5b 片。"
    )

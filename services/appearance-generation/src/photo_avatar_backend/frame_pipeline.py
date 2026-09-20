# -*- coding: utf-8 -*-
"""写实风（`frame-video-v1`）两个 step 的服务侧实现。

    generateMotionSource   照片 → 看照片（gpt-4o）→ 透明母版 → 绿幕首帧 → 视频  （**要花钱**）
    packFrameSequence      scratch 里的 mp4 → 抠像 → 验收 → WebP → zip          （**0 算力**）

两个 step 按「钱」切：视频失败要重付**整支视频的钱**（单价见下面 `VIDEO_VERSION` 那段），
粒度不能太粗；而后面半段（抠像/验收/打包）0 算力，便宜到可以整段重跑。

「看照片」那一步（`analyze_photo_facts`）没有自己的 step 名 —— 它是
`generateMotionSource` 的**内部第一步**。理由见该函数的 docstring：它失败只意味着
「母版提示词少一句毛长提示」，不该有能力让整单失败。

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
import shutil
import subprocess
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .contracts import FrameStepRequest
# `frames.action_prompts` / `frames.prompts` 只依赖 json/re/pathlib（**没有 numpy**），
# 所以可以放在模块顶层；与 `frames.pipeline` 不同 —— 那个在函数里懒 import，
# 免得后端启动就硬依赖 numpy/PIL/scipy。
from .frames.action_prompts import (
    ACTION_IDS,
    ActionFacts,
    FALLBACK_ACTION_FACTS,
    action_first_frame_master,
    action_frame_range,
    action_frame_target,
    action_hold_range,
    joins_idle_schedule,
    load_action,
    render_action_prompt_for,
    uses_end_frame,
)
from .frames.prompts import render_loop_prompt
from .lk888_client import Lk888Error

SCRATCH_DIR = "scratch"
MOTION_SOURCE_FILE = "motion-source.mp4"
# 动作视频放 idle 那支旁边的子目录：`scratch/<providerSessionId>/actions/<actionId>.mp4`。
#
# ⚠️ **idle 的路径刻意不搬**（仍是 `motion-source.mp4`）：后端重启后重试要命中它，
# 而且「把旧会话的 mp4 拷到新会话的同一路径」这个省钱手法完全依赖这个固定名字。
ACTION_VIDEO_SUBDIR = "actions"
PACK_SUBDIR = "pack-frame-sequence"
# 母版/首帧的中间产物与 mp4 同级，方便出问题时整目录打包回看
MOTION_SUBDIR = "motion-source"
MASTER_DIR = "00-母版"
FIRST_FRAME_DIR = "01-首帧"
# 「动作专用母版」的家：`scratch/<providerSessionId>/lift-master/<actionId>/`。
# 与 idle 的 `motion-source/` 平级 —— 它是另一条产线（参考图是 idle 母版、不是照片），
# 且按动作分子目录：将来若有第二支动作要自己的首帧，不会互相串。
LIFT_MASTER_SUBDIR = "lift-master"

# 产品固定 WebP（与内置宠物 04/05 的现有资产一致）。
FRAME_FORMAT = "webp"

# ---------------- 写实风的成套规格（改这些等于改产品，不是调参） ----------------
#
# 计费：模型固定成 `seedance-2.0-guanfang-anmiao`（**只有官方特惠一个渠道、按秒**），
# 所以单价**可预估**：秒 × 档位价。2026-09-15 实测 **Mini + 480p + 12s = 2.8416 算力/支**
# （标准 + 同规格 = 5.6832）。加上母版 0.05~0.16、gpt-4o 看照片判毛长 ~0.004
# ⇒ **一只宠（四支视频）≈ 11.4 算力**。
# （换模型名白名单见 `config._fixed_model`；`audit._MODELS` 只能追加。）
#
# ⚠️ **Mini 档会把整帧压暗**（实测：主体亮度中位 226→200、饱和 16→25、背景绿 G 255→168），
# 白猫肉眼变灰。所以 `packFrameSequence` 那条路**必须同时开校色**
# （`color_match=<母版>`，见本文件里的 pack_frame_sequence）—— 实测校色后主体距母版
# 4.2~4.4，反而比标准档不校色（4.5~5.1）更贴。
# 时长 12s / 24fps / 42ms 与内置资产同规格，换数值会让新宠物的节奏与内置不一致。
VIDEO_VERSION = "Mini"
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

# ---------------- 看照片（gpt-4o）：只补母版提示词那一句毛长 ----------------
#
# 母版提示词里有个 `{{COAT_LEN}}` 位置（short-haired / long-haired）。它**不是必填**：
# 提示词下一段本来就要求「毛长照照片一模一样」，毛长真正的来源是照片。但这句话对
# **长毛猫**值钱 —— 没有它，母版容易把围脖和尾巴画短。
#
# 产品里问不出毛长（`FrameStepRequest` 没有这个字段），所以让 gpt-4o 看一眼照片。
# ⚠️ **它是一次「可选」调用**：读不出来就返回 `unknown`，**不猜**（与 `analyzeIdentity`
# 那条「不得推断缺失 trait」的纪律一致）。见 `PhotoFacts`。
#
# 改这段提示词等于改产品产出（所有新宠的母版都从它过），别手滑。
PHOTO_FACTS_SPECIES = ("cat", "dog")
PHOTO_FACTS_COAT = ("short", "long")
# 「看不出来」用哨兵字符串，不用 `["string","null"]` 联合类型：严格模式的 json_schema
# 对联合类型支持参差，哨兵值在两边都只是普通字符串。
UNKNOWN_FACT = "unknown"

_PHOTO_FACTS_PROMPT = (
    "Look only at the supplied pet photos, and report only what is directly visible in them. "
    'Do not guess, do not use breed knowledge: when something is not observable, answer "unknown". '
    "species: the animal the photos actually show. "
    'coat: "short" when the fur lies flat against the body with crisp outlines; '
    '"long" when the fur is visibly fluffy, or there is a ruff, a plume tail or long ear tufts; '
    'answer "unknown" when the coat length cannot be judged from these photos.'
)


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
class ActionVideo:
    """一条偶发/交互动作的绿幕视频。**留在服务侧，不往客户端送任何字节。**

    动作各是**独立的一支视频**（呼吸+眨眼+摇尾焊死在 idle 那一支里，不动），
    所以它们各自复用、各自算钱。
    """

    action_id: str
    video_path: Path
    video_task_id: str | None
    video_bytes: int
    reused: bool

    def to_wire(self) -> dict[str, object]:
        return {
            "actionId": self.action_id,
            "videoBytes": self.video_bytes,
            "videoTaskId": self.video_task_id,
            "reused": self.reused,
        }


@dataclass(frozen=True)
class MotionSource:
    """`generateMotionSource` 的产物。

    **没有 artifact** —— 这一步的产物是留在服务侧的几支 mp4（idle 一支 + 每个动作一支），
    不往客户端送任何字节。客户端要的只是「成了没有」+「花了哪几次」。

    `master_path` / `first_frame_path` 在**整步复用**时为 `None`：idle 与全部动作都已在
    scratch 里时不会重跑母版与首帧（那才是「不重付」的意义）；只补动作时会去 scratch
    捡回母版、重算首帧（母版是钱、首帧是免费的）。

    `actions` 只放**真做出来的**那些 —— 某支失败了就被跳过（动作是加分项，
    拖垮整步会让用户连基础宠物都拿不到），因此「少了哪支」看这个列表就知道。
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
    actions: tuple[ActionVideo, ...] = ()

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
            "actions": [action.to_wire() for action in self.actions],
        }


@dataclass(frozen=True)
class PhotoFacts:
    """gpt-4o 看一眼照片后能说的那两件事。

    **`None` 一律表示「照片里看不出来」，不表示「失败了」。** 判不出来就不提 ——
    提示词里少一句是安全的，猜错一句会让母版照着错的那句画。
    """

    species: str | None
    coat: str | None


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


def action_video_path(state_dir: Path, provider_session_id: str, action_id: str) -> Path:
    """一支动作视频的位置（与 idle 的 mp4 同一个 scratch 目录）。

    **逐支独立**：`actions/yawn.mp4` 在不在，决定 yawn 要不要重新生成 ——
    与 idle 那支的复用互不影响。所以「拷一支旧 idle mp4 到新会话」时，
    idle 白捡、只补动作的钱。
    """
    return scratch_dir(state_dir, provider_session_id) / ACTION_VIDEO_SUBDIR / f"{action_id}.mp4"


def _has_video(path: Path) -> bool:
    """「有一支能用的视频」。

    **非空**才算 —— 半支（`.part` 残留、写盘被打断）会被下游当成完整的往下跑，
    表现是「桌宠卡住不动」或抠像报一句看不懂的错。`_generate_video` 是原子落盘，
    所以正常不会出现半支；这里是防「上一个人手工拷进来的东西」。
    """
    return path.is_file() and path.stat().st_size > 0


def analyze_photo_facts(
    request: FrameStepRequest,
    *,
    client: Any,
    log: Callable[[str], None] = print,
) -> PhotoFacts:
    """看一眼照片：判物种与毛长档位。

    **没有自己的 step 名**，是 `generateMotionSource` 的内部第一步。这样就不必动
    那三处 step 白名单（`contracts._FRAME_STEPS` / `job_store._STEPS` / 客户端），
    也不会多出一次「能重试的独立进度」。

    两种「不知道」在这里被抹平成同一个结果（`None`），因为它们对下游是同一件事：
    - 照片里看不出毛长（`unknown`）；
    - 模型没按 schema 回（例如 `"medium"`）。

    `species` 与请求不一致时**只记日志**，不改行为：物种是产品写进 manifest 的字段，
    不该被一次模型判断推翻；但两者不一致是「照片传错了」的信号，值得留痕。
    """
    response = client.analyze_json(
        _PHOTO_FACTS_PROMPT, [image.png for image in request.source_images], _photo_facts_schema()
    )
    if not isinstance(response, Mapping):
        raise FramePipelineError("照片分析没有返回对象")
    coat = _photo_fact(response, "coat", PHOTO_FACTS_COAT, log)
    species = _photo_fact(response, "species", PHOTO_FACTS_SPECIES, log)
    if species is not None and species != request.species:
        log(f"[照片分析] ⚠️ 照片看着像 {species}，请求里写的是 {request.species}（只记日志，不改行为）")
    log(f"[照片分析] 物种={species or UNKNOWN_FACT}  毛长={coat or UNKNOWN_FACT}")
    return PhotoFacts(species=species, coat=coat)


def _photo_facts_schema() -> dict[str, Any]:
    return {
        "type": "object",
        "additionalProperties": False,
        "properties": {
            "species": {"type": "string", "enum": ["cat", "dog", UNKNOWN_FACT]},
            "coat": {"type": "string", "enum": ["short", "long", UNKNOWN_FACT]},
        },
        "required": ["species", "coat"],
    }


def _photo_fact(
    response: Mapping[str, Any],
    key: str,
    allowed: tuple[str, ...],
    log: Callable[[str], None],
) -> str | None:
    value = response.get(key)
    if value == UNKNOWN_FACT:
        return None
    if value not in allowed:
        log(f"[照片分析] {key} 是意料之外的值 {value!r}（当成看不出来）")
        return None
    return value


# 动作提示词里那 4 个「因猫而异」的字段的判定提示。
#
# 与毛长那条（`_PHOTO_FACTS_PROMPT`）同一个风格：**只描述看得见的**，
# 看不出来就答 `unknown`。`coat_guard` / `coat_negative` 是**从刚判出的毛色推出来的**
# （「这种颜色最容易往哪漂」），不是凭空发明的额外知识。
_ACTION_FACTS_PROMPT = (
    "You are looking at one green-screen frame of a single pet, taken for a video shoot. "
    "Describe only what is directly visible in THIS frame; do not guess, do not use breed "
    'knowledge. Answer "unknown" for any field you cannot judge from the frame. '
    "identity: one short phrase for the pet's face, head and body shape (shape only, no colour). "
    "coat: one short phrase for the fur colour and markings — dominant colour first, then the "
    "accent colour, with approximate RGB values when they are clear. "
    "coat_guard: for exactly that coat, the colour drifts to forbid (e.g. turning grey, "
    "yellowing, being washed out or over-brightened, losing the dark markings). "
    "coat_negative: the same drifts as a comma-separated list of english keywords."
)


def _action_facts_schema() -> dict[str, Any]:
    fields = ("identity", "coat", "coat_guard", "coat_negative")
    return {
        "type": "object",
        "additionalProperties": False,
        "properties": {field: {"type": "string"} for field in fields},
        "required": list(fields),
    }


def analyze_action_facts(
    client: Any,
    frame_png: bytes,
    *,
    log: Callable[[str], None] = print,
) -> ActionFacts:
    """看一眼**绿幕首帧图**，判动作提示词要的那 4 个字段。

    为什么用首帧图而不是原照片：动作视频是从首帧图开始的，**首帧图才是「整段视频
    长什么样」的锚**；而且它是绿幕版、背景干净，判定比生活照稳。

    形状与纪律照 `analyze_photo_facts`（毛长那条）：

    - **没有自己的 step 名**，是内部一步（片 3 接进 `generateMotionSource`）。
    - 🔴 **只能降级，不能失败。** 这四项是提示词的**补充强调** —— 真正的身份锚是首帧图
      本身（骨架第一句就写着「以首帧图作为唯一的身份/造型/配色参考」），少它们照样出得来；
      而一次失败会带走后面 ~5 算力/支的视频。一个纯提质项不该有这个权力。
    - 四种「不可用」抹平成同一个结果（**整包兜底**）：答 `unknown`、答非所问、缺字段、
      回包不成形。**整包**而不是逐字段，因为兜底本身就是自洽的一套措辞，
      混用「真判的 identity + 兜底的 coat」只会自相矛盾。
    - `AttributeError` 之类的**代码错误不在吞的范围内**：那是 bug，要炸出来。
      （所以 `_FakeClient` 必须真的实现 `analyze_json`。）
    """
    try:
        response = client.analyze_json(_ACTION_FACTS_PROMPT, [frame_png], _action_facts_schema())
    except (Lk888Error, FramePipelineError) as error:
        log(f"[动作分析] 降级为通用兜底（{type(error).__name__}: {error}）")
        return FALLBACK_ACTION_FACTS
    if not isinstance(response, Mapping):
        log("[动作分析] 回包不是对象 → 降级为通用兜底")
        return FALLBACK_ACTION_FACTS

    values: dict[str, str] = {}
    for field in ("identity", "coat", "coat_guard", "coat_negative"):
        value = response.get(field)
        text = value.strip() if isinstance(value, str) else ""
        if not text or text == UNKNOWN_FACT:
            log(f"[动作分析] {field} 不可用（{value!r}）→ 降级为通用兜底")
            return FALLBACK_ACTION_FACTS
        values[field] = text
    log(f"[动作分析] 判定成功：identity={values['identity'][:40]!r} coat={values['coat'][:40]!r}")
    return ActionFacts(**values, detected=True)


def generate_motion_source(
    request: FrameStepRequest,
    *,
    client: Any,
    state_dir: Path,
    report_task_id: Callable[[str], None] | None = None,
    log: Callable[[str], None] = print,
) -> MotionSource:
    """`generateMotionSource`：照片 → 看照片 → 母版 → 绿幕首帧 → 绿幕视频。

    **唯一花钱的 step**（≈5.75 算力/次）。三条防线保证不白花：

    1. **已有 mp4 就直接复用**（不重跑、不重付）。后端重启会清掉 job 结果
       （`_load_state` 把非 `renderTextureAtlas` 的成功态标 failed），这时候客户端
       用同一个 `providerSessionId` 重试，就能白捡回那支视频。
       要真重跑就换一个 `providerSessionId`（= 用户点「重新生成」）。
    2. **取景收敛在免费阶梯上做完再进视频**：首帧不烧算力，两个余量不达标就调小
       `scale` 重出，全挂才报错 —— 长毛猫当初没做这一步，白花 4.67 算力。
    3. **上游错误码原样透出**：`contentPolicy` 这类不可重试的错误必须让上层看见，
       否则会自动重试同一份提示词、必然再被拒一次。

    「看照片」那一步**只能降级、不能失败**（见 `_coat_hint`）。复用分支在它之前 ——
    复用不重跑，也就连这一次 gpt-4o 都不打。
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

    idle_path = motion_source_path(state_dir, provider_session_id)
    idle_reused = _has_video(idle_path)
    pending_actions = [
        action_id
        for action_id in ACTION_IDS
        if not _has_video(action_video_path(state_dir, provider_session_id, action_id))
    ]
    if idle_reused and not pending_actions:
        log(
            f"[复用] scratch 里已有 {MOTION_SOURCE_FILE} + 全部动作视频，整步跳过（不重付算力）"
        )
        return _reused_motion_source(
            idle_path,
            actions=tuple(
                _reused_action_video(action_video_path(state_dir, provider_session_id, action_id))
                for action_id in ACTION_IDS
            ),
        )

    work_dir = scratch_dir(state_dir, provider_session_id) / MOTION_SUBDIR / (
        f"attempt-{request.attempt}"
    )
    # 一个 job 的 `lk888_task_id` 是单选，多支视频只能报一支（见 `_report_first`）。
    report_first = _report_first(report_task_id)

    # ---- 基座：母版（钱，能复用就复用）+ 首帧（免费阶梯）----
    # `coat` 提到这一层是为了「动作专用母版」（它的参考图就是这张母版）；
    # 母版被复用时它保持 `None` = 「不知道毛长」，与 `render_master_prompt` 的
    # 既有处理一致（少一句话不影响产出）。
    coat: str | None = None
    master_path = _find_existing_master(state_dir, provider_session_id)
    if master_path is not None:
        log(f"[复用] scratch 里已有母版 {master_path.name}，跳过母版（不重付 0.06 算力）")
        master_task_id: str | None = None
    else:
        log("[1/4] 看照片（gpt-4o）：判毛长档位，给母版提示词补一句")
        coat = _coat_hint(request, client=client, log=log)
        log("[2/4] 生成透明母版（gpt-image-2，约 0.06 算力）")
        master_path, master_task_id = _generate_master(
            client=client, request=request, coat=coat, work_dir=work_dir, log=log
        )

    log("[3/4] 绿幕首帧 + 取景收敛（免费阶梯，不进视频）")
    fitted = _converge_first_frame(master_path, work_dir=work_dir, state_dir=state_dir, log=log)

    # ---- idle：基础，必须有。动作全部挂在它上面 ----
    if idle_reused:
        video_task_id: str | None = None
        log(f"[4/4] idle 视频复用 scratch 里的 {MOTION_SOURCE_FILE}")
    else:
        # 不在日志里写单价：价格随档位/渠道漂，写死必然过期（规格与单价见 `VIDEO_VERSION` 那一段）。
        log(f"[4/4] 生成 idle 绿幕视频（{VIDEO_VERSION} / {VIDEO_RESOLUTION} / {VIDEO_DURATION}s）")
        video_task_id = _generate_video(
            client=client,
            first_frame=fitted.frame_png,
            video_path=idle_path,
            prompt=render_loop_prompt(),
            report_task_id=report_first,
            label="idle 视频",
            log=log,
        )

    # ---- 动作：加分项，某支失败只跳过那一支 ----
    actions = _action_videos(
        request=request,
        client=client,
        state_dir=state_dir,
        provider_session_id=provider_session_id,
        first_frame=fitted.frame_png,
        idle_master=master_path,
        coat=coat,
        report_task_id=report_first,
        log=log,
    )

    return MotionSource(
        out_dir=work_dir,
        video_path=idle_path,
        master_path=master_path,
        first_frame_path=fitted.frame_png,
        master_task_id=master_task_id,
        video_task_id=video_task_id,
        first_frame_scale=fitted.scale,
        first_frame_left_margin=fitted.left_margin,
        first_frame_right_margin=fitted.right_margin_at_full_swing,
        video_bytes=idle_path.stat().st_size,
        reused=False,
        actions=actions,
    )


def _report_first(
    report: Callable[[str], None] | None,
) -> Callable[[str], None] | None:
    """把「第一支提交的视频」报给 `job_store`，后面的丢弃。

    `lk888_task_id` 在 job 上**是单选**（上游删除要用它），报第二个不同 id 会被
    `job_store` 拒掉并让整个 step 失败。多支视频时这是个妥协：上游删除只能取消一支。

    idle 最先跑，所以正常情况下报出去的就是 idle 那支（**语义与单支时完全一致**）；
    只有当 idle 被复用、只补动作时，报出去的才是一支动作 —— 那也比什么都不报更诚实。
    """
    if report is None:
        return None
    state = {"reported": False}

    def report_first(task_id: str) -> None:
        if state["reported"]:
            return
        state["reported"] = True
        report(task_id)

    return report_first


def _reused_action_video(video_path: Path) -> ActionVideo:
    return ActionVideo(
        action_id=video_path.stem,
        video_path=video_path,
        video_task_id=None,
        video_bytes=video_path.stat().st_size,
        reused=True,
    )


def _find_existing_master(state_dir: Path, provider_session_id: str) -> Path | None:
    """在 scratch 里找**已经算过的母版**（文件名带 task id，所以只能 glob）。

    为什么值得找：idle 那支 mp4 复用掉、但动作还缺时，动作视频仍然需要首帧图，
    而首帧 = 母版 + **免费的**取景阶梯。留住这个母版 ⇒ 这条路上只有动作要花钱。
    """
    root = scratch_dir(state_dir, provider_session_id) / MOTION_SUBDIR
    if not root.is_dir():
        return None
    for candidate in sorted(root.glob(f"attempt-*/{MASTER_DIR}/*.png")):
        if candidate.stat().st_size > 0:
            return candidate
    return None


def lift_master_dir(state_dir: Path, provider_session_id: str, action_id: str) -> Path:
    """动作专用母版的家：`scratch/<providerSessionId>/lift-master/<actionId>/`。"""
    return scratch_dir(state_dir, provider_session_id) / LIFT_MASTER_SUBDIR / action_id


def _find_existing_lift_master(
    state_dir: Path, provider_session_id: str, action_id: str
) -> Path | None:
    """`_find_existing_master` 的动作版：scratch 里已经算过的那张专用母版。

    为什么值得找：专用母版是**花钱的**（gpt-image-2 图生图 ≈0.06），而取景收敛是
    免费的 —— 留住母版，之后重跑这一步就不必再付这 0.06。复用它还有第二个好处：
    **老王验收过的那张**不会被模型重新采样成另一只猫（同一份提示词两次出图不保证一样）。
    """
    root = lift_master_dir(state_dir, provider_session_id, action_id)
    if not root.is_dir():
        return None
    for candidate in sorted(root.glob("*.png")):
        if candidate.stat().st_size > 0:
            return candidate
    return None


def _action_video_first_frame(
    action: dict,
    *,
    action_id: str,
    request: FrameStepRequest,
    first_frame: Path,
    idle_master: Path | None,
    coat: str | None,
    client: Any,
    state_dir: Path,
    provider_session_id: str,
    log: Callable[[str], None],
) -> Path:
    """这一支动作该用哪张首帧。

    没声明 `firstFrameMaster` ⇒ 直接退回 `first_frame`（= idle 那张）。
    声明了 ⇒ **图生图另做一张**：参考图是 idle 的**透明母版**（不是绿幕首帧 ——
    要的是干净身份、不带绿幕色偏），只改姿态，再走同一套免费取景收敛。

    抛出去就是「这一支放弃」：调用方与「视频生成失败」同样处理。🔴 **绝不退回
    idle 首帧** —— 那等于又拿坐姿演拎起，正是这一整套要消灭的行为。
    """
    prompt_file = action_first_frame_master(action)
    if prompt_file is None:
        return first_frame

    if idle_master is None:
        # 只有 idle 母版被复用、又没找到它时才会走到这。没有身份参考而凭空生成一张
        # 姿态母版 = 换一只猫，所以宁可让这一支缺掉。
        raise FramePipelineError(
            f"{action_id} 需要专用首帧，但这只宠物没有 idle 透明母版可作参考"
        )

    master = _find_existing_lift_master(state_dir, provider_session_id, action_id)
    if master is not None:
        log(f"[{action_id} 首帧] 复用 scratch 里的 {master.name}（不重付 0.06 算力）")
    else:
        from .frames.prompts import render_master_prompt

        prompt = render_master_prompt(request.species, coat, template=prompt_file)
        log(f"[{action_id} 首帧] 生成专用姿态母版（gpt-image-2 图生图，约 0.06 算力）")
        task_id = client.submit_image(prompt, [idle_master.read_bytes()])
        state = _wait_for_media(client, task_id, label=f"{action_id} 专用母版")
        out_dir = lift_master_dir(state_dir, provider_session_id, action_id)
        out_dir.mkdir(parents=True, exist_ok=True)
        master = out_dir / f"母版-{task_id}.png"
        master.write_bytes(client.download(state.result_url))
        log(f"[{action_id} 首帧] 母版 task={task_id}  {master.stat().st_size // 1024} KB")

    fitted = _converge_first_frame(master, work_dir=master.parent, state_dir=state_dir, log=log)
    return fitted.frame_png


def _action_videos(
    *,
    request: FrameStepRequest,
    client: Any,
    state_dir: Path,
    provider_session_id: str,
    first_frame: Path,
    idle_master: Path | None,
    coat: str | None,
    report_task_id: Callable[[str], None] | None,
    log: Callable[[str], None],
) -> tuple[ActionVideo, ...]:
    """逐支生成/复用偶发与交互动作视频。

    四条规矩：

    1. **逐支复用**：某支已在 scratch 就跳过它（与 idle 那支同一套依据）——
       「拷一支旧 idle mp4 到新会话」时，idle 白捡、只付动作的钱。
    2. 🔴 **某支失败只跳过那一支，不拖垮整步**：动作是加分项，一次审核误伤
       （`contentPolicy` 不可重试）不该让用户连基础宠物都拿不到。
       缺了哪支看 `MotionSource.actions` 就知道。
    3. **身份锚是首帧图**：动作视频与 idle 用**同一张首帧图**、同一个画幅 ——
       这就是「触发动作时不跳变」的前提；取景框的复用是抠像那一步的事
       （`--no-autocrop` + ref-params），不在视频生成这一步。
       ⚠️ 例外见第 4 条。
    4. 🔴 **声明了 `firstFrameMaster` 的动作改用「自己的首帧」**（目前只有拎起）：
       **首帧定义了姿态起点** —— 让坐姿首帧里的猫靠语言变成四爪离地，模型给不出
       可靠结果（2026-09-20 四次实测：顶边 / 劈叉 / 站起来走两步）。建国就是靠
       一张专门的「背弓悬垂」母版做到的，这里把它产线化。
       这一支**绝不退回 idle 首帧**：退回 = 又用坐姿演拎起，正是要消灭的行为。
    """
    pending = [
        action_id
        for action_id in ACTION_IDS
        if not _has_video(action_video_path(state_dir, provider_session_id, action_id))
    ]
    if not pending:
        log(f"[动作] scratch 里已有全部 {len(ACTION_IDS)} 支动作视频，跳过（不重付算力）")
        return tuple(
            _reused_action_video(action_video_path(state_dir, provider_session_id, action_id))
            for action_id in ACTION_IDS
        )

    # 只在**真要生成**时才判字段（判不出会退到通用兜底，见 `analyze_action_facts`）。
    facts = analyze_action_facts(client, first_frame.read_bytes(), log=log)
    log(f"[动作] 待生成 {len(pending)} 支：{'、'.join(pending)}")

    videos: list[ActionVideo] = []
    for action_id in ACTION_IDS:
        video_path = action_video_path(state_dir, provider_session_id, action_id)
        if _has_video(video_path):
            log(f"[动作] {action_id} 复用 scratch 里的视频")
            videos.append(_reused_action_video(video_path))
            continue

        action = load_action(action_id)
        prompt = render_action_prompt_for(action_id, facts, pet_id=request.pet_id)
        try:
            action_frame = _action_video_first_frame(
                action,
                action_id=action_id,
                request=request,
                first_frame=first_frame,
                idle_master=idle_master,
                coat=coat,
                client=client,
                state_dir=state_dir,
                provider_session_id=provider_session_id,
                log=log,
            )
            task_id = _generate_video(
                client=client,
                first_frame=action_frame,
                video_path=video_path,
                prompt=prompt,
                report_task_id=report_task_id,
                label=f"动作 {action_id}",
                end_frame=uses_end_frame(action),
                log=log,
            )
        except (Lk888Error, FramePipelineError) as error:
            log(f"[动作] {action_id} 没做出来，跳过这一支：{type(error).__name__}: {error}")
            continue
        videos.append(
            ActionVideo(
                action_id=action_id,
                video_path=video_path,
                video_task_id=task_id,
                video_bytes=video_path.stat().st_size,
                reused=False,
            )
        )
    return tuple(videos)


def upload_variant_id(session_id: str, revision: int) -> str:
    """上传生成的宠物包用的 `variantId`。

    ⚠️ 这个拼法必须与 Rust 侧 `finalization.rs::photo_avatar_record` 里的
    `variant_id` **逐字一致**：安装时 finalization 拿它与 manifest 里的 `variantId`
    比对，不一致就报 `... preview identity does not match finalization`
    （第一次真跑到「接受并安装」时才炸，前面全绿）。

    `frames.packing` 的默认值 `combo-loop-v1` 是**内置宠物**那条路的常量：内置包
    不经过 finalization，variantId 只是「一个变体」的名字。上传生成的包要走
    finalization，它按「哪次会话的第几版」标识变体 —— 所以这里不能沿用默认值。
    """
    return f"photo-avatar-{session_id}-{revision}"


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
    clips = _action_clips(state_dir, provider_session_id)
    if clips:
        log(
            f"[packFrameSequence] 另有 {len(clips)} 支动作并进同一个包："
            f"{'、'.join(clip.action_id for clip in clips)}"
        )
    # 校色参考 = 那张透明**母版**（跟用户照片对齐的那张）。
    #
    # 为什么必须给 idle 也校色：**Mini 档会把整帧压暗**（主体亮度中位 226→200、饱和 16→25），
    # 白猫肉眼变灰。实测校色把主体拉回距母版 4.2~4.4（不校色是 33.8~40.2）。
    #
    # ⚠️ 动作**不用改**：它们拿的是 idle 抠像后的第 0 帧（`pipeline.IDLE_ANCHOR_FRAME`），
    # 而那一帧此时已经是校色过的 ⇒ 动作自动跟着校到同一个目标，两支不会色偏。
    #
    # 找不到母版（scratch 被清过）就**降级不校色**，不要因为校色参考缺失而让打包失败：
    # 那一步 0 算力、不该成为「已付费视频装不进去」的理由。
    master = _find_existing_master(state_dir, provider_session_id)
    log(f"[packFrameSequence] 校色参考={'母版 ' + master.name if master else '无（跳过校色）'}")
    build = build_frame_sequence(
        video,
        out_dir,
        pet_id=request.pet_id,
        display_name=request.display_name,
        species=request.species,
        variant_id=upload_variant_id(request.session_id, request.revision),
        color_match=master,
        path_base=state_dir,
        action_clips=clips,
        log=log,
    )
    return FrameSequenceArtifact.from_build(build)


def _action_clips(state_dir: Path, provider_session_id: str) -> tuple[Any, ...]:
    """scratch 里已生成的动作视频 → 打包用的动作清单。

    **扫目录，不信请求里的清单** —— 与 idle 那支同一个真源：scratch 里有什么就是什么。
    生成阶段失败的支不会留文件，所以这里天然只拿到成功的那些。

    每支的**打包规格来自它自己的动作配置**（`assets/motion-prompts/actions/*.json`）：
    进不进 `idleSchedule` 由 `pipelineTail` 决定（交互动作是 `--no-idle-schedule`），
    不在这里另抄一份规则。

    **精修参数也来自动作配置**：`holdRange`（悬空保持窗口）+ `frameRange`/`frameTarget`
    （裁掉静止废料 + 重采样到内置宠的节奏），三者都用**原始抠像帧**下标，
    映射成包内坐标是打包层的事 —— 见 `docs/设计/动作精修流程-2026-09-20.md`。
    """
    # 懒 import：`frames.pipeline` 会拉起 numpy/PIL/scipy（理由见上面那段注释）。
    from .frames.pipeline import ActionClip

    clips: list[ActionClip] = []
    for action_id in ACTION_IDS:
        path = action_video_path(state_dir, provider_session_id, action_id)
        if not _has_video(path):
            continue
        action = load_action(action_id)
        clips.append(
            ActionClip(
                action_id=action_id,
                video=path,
                # 偶发与交互动作都是非循环：播完就回 idle（内置 04/05 的 yawn/lick/
                # grab-release 全是 loop=false），所以这不是每支配置的字段。
                loop=False,
                # **人定的**「悬空保持」区间；配置里没有就是不写这个键（见 action_hold_range）。
                hold_range=action_hold_range(action),
                scheduled=joins_idle_schedule(action),
                # **人定的**精修：裁废料 + 重采样（见 action_frame_range / action_frame_target）。
                frame_range=action_frame_range(action),
                frame_target=action_frame_target(action),
            )
        )
    return tuple(clips)


# ------------------------------------------------------------------ 内部实现


@dataclass(frozen=True)
class _FittedFirstFrame:
    frame_png: Path
    scale: float
    left_margin: float
    right_margin_at_full_swing: float


def _reused_motion_source(
    video_path: Path, *, actions: tuple[ActionVideo, ...] = ()
) -> MotionSource:
    """整步复用：idle 与全部动作视频都已在 scratch 里（一次 API 都不打）。"""
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
        actions=actions,
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


def _coat_hint(
    request: FrameStepRequest,
    *,
    client: Any,
    log: Callable[[str], None],
) -> str | None:
    """母版提示词的毛长档位。**这一步只能降级，不能失败。**

    少一句「long-haired」母版照样出得来（提示词下一段本来就要求「毛长照照片一模一样」），
    但一次失败会带走后面那 5.02 算力的视频 —— 一个纯省钱的可选步骤不该有这个权力。
    所以上游故障（`Lk888Error`：网络 / 额度 / 审核 / 协议）一律咽掉、只记日志。

    `AttributeError` 之类的**代码错误不在此列**：那是 bug，要炸出来。
    """
    try:
        facts = analyze_photo_facts(request, client=client, log=log)
    except (Lk888Error, FramePipelineError) as exc:
        log(f"[照片分析] 跳过毛长档位（照常出母版）：{exc}")
        return None
    return facts.coat


def _generate_master(
    *,
    client: Any,
    request: FrameStepRequest,
    coat: str | None,
    work_dir: Path,
    log: Callable[[str], None],
) -> tuple[Path, str]:
    """照片 → 透明母版。

    `coat` 来自 `analyze_photo_facts`，可能是 `None`（照片里看不出毛长）——
    此时渲染不带档位的那一版，安全：`{{COAT_LEN}}` 只是一句提前的提示，
    毛长真正的来源是照片本身。

    物种**一律用请求里的**，不用分析结果 —— 那是产品写进 manifest 的字段，
    两者不一致时 `analyze_photo_facts` 会记一条日志，但不改行为。
    """
    from .frames.prompts import render_master_prompt

    prompt = render_master_prompt(request.species, coat)
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
    prompt: str,
    report_task_id: Callable[[str], None] | None,
    log: Callable[[str], None],
    label: str = "视频",
    end_frame: bool = False,
) -> str:
    """首帧（+ 可选尾帧）+ 提示词 → 绿幕视频，原子落盘到约定的 mp4 路径。

    `prompt` 由调用方给：idle 用 `render_loop_prompt()`，动作各自用
    `render_action_prompt_for()` —— **这一层不认识提示词**，只负责发与收。

    `end_frame=True`（交互动作 `grab-release`）时**同一张图传两次**：
    它必须首尾回到同一张端坐图，否则拎起来落不回原姿态。

    ⚠️ **只上报一支的 task id**（见 `_report_first`）：job 上的 `lk888_task_id` 是单选，
    报第二个不同 id 会被 `job_store` 拒掉并让整个 step 失败。母版那一个记在
    `MotionSource.master_task_id`、动作那几支记在各自的 `ActionVideo` 里，追溯够用。
    """
    frame = first_frame.read_bytes()
    images = [frame, frame] if end_frame else [frame]
    task_id = client.submit_video(
        prompt,
        images=images,
        version=VIDEO_VERSION,
        duration=VIDEO_DURATION,
        resolution=VIDEO_RESOLUTION,
        aspect_ratio=VIDEO_ASPECT_RATIO,
        mode=VIDEO_MODE,
    )
    if report_task_id is not None:
        report_task_id(task_id)
    log(f"[{label}] task={task_id}，开始轮询（最多 {MAX_WAIT_SECONDS:.0f}s）")

    state = _wait_for_media(client, task_id, label=label)
    payload = client.download_video(state.result_url)

    # 先写 .part 再原子改名：中途崩了不会留下一支「看起来存在、其实截断」的 mp4 ——
    # `packFrameSequence` 只判断 video.is_file()，半支视频会被当成完整的往下跑。
    video_path.parent.mkdir(parents=True, exist_ok=True)
    staging = video_path.with_name(video_path.name + ".part")
    staging.write_bytes(payload)
    staging.replace(video_path)
    log(f"[{label}] {video_path.name}  {len(payload) // 1024} KB")
    _strip_audio_track(video_path, log=log)
    return task_id


# 音轨白白占体积：实测一支 12s / 480p 的 mp4 里 AAC 音轨 **126,550 bps ≈ 191 KB**，
# 占整支 3,980 KB 的 **4.8%**。
#
# ⚠️ 别指望关音频省钱 —— **平台侧省不了**：
#   · `seedance-2.0-guanfang-anmiao` 是**按秒计费**（`秒 × 档位价`），音频不单独计价；
#   · `seedance-2.0-guanfang` 的 pricing 里也只有 `output_token_price` 一项，没有音频项。
#   → 收益只有「下载体积 / 磁盘占用」和「产物里不该有跟画面无关的东西」。
# ⚠️ 也**没法在请求里关**：media 协议的 `params` 表里没有音频开关
# （只有 `audio_url` = 参考音频**输入**）。所以只能下载后剥。
def _strip_audio_track(video_path: Path, *, log: Callable[[str], None]) -> None:
    """原地去掉音轨：`-c copy` 只重封装，**视频流逐字节不变**（帧内容零影响）。

    失败**不阻断** —— 音轨不影响抠像，别为它丢掉一支已经付过钱的视频。
    """
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        log("[警告] 找不到 ffmpeg，跳过剥音轨（不影响抠像）")
        return
    # 输出名**必须留 `.mp4` 后缀**：ffmpeg 按扩展名挑 muxer，写成 `xxx.noaudio`
    # 会直接 `Unable to find a suitable output format`（2026-09-15 踩到）。
    # 同时显式 `-f mp4`，不靠猜。
    stripped = video_path.with_name(video_path.stem + ".noaudio.mp4")
    result = subprocess.run(
        [ffmpeg, "-y", "-loglevel", "error", "-i", str(video_path),
         "-c", "copy", "-an", "-f", "mp4", str(stripped)],
        capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    if result.returncode != 0 or not stripped.is_file():
        log(f"[警告] 剥音轨失败，保留带音轨的原样：{result.stderr[-500:]}")
        stripped.unlink(missing_ok=True)
        return
    # 先剥到 .noaudio 再改名：中途被杀时 video_path 仍是完整的原文件
    before, after = video_path.stat().st_size, stripped.stat().st_size
    stripped.replace(video_path)
    log(f"[音轨] 已剥除  {before // 1024} KB -> {after // 1024} KB（省 {before - after} B）")

"""把一帧序列目录打包成 frame-sequence-v1（schema 7）运行时包。

从 `scripts/poc_组合循环打包.py` 原样搬进服务（算法零改动，只加了入参校验与
「拒绝覆盖非空目录」的显式开关）。脚本现在只是这里的薄 CLI 包装。

**单动作与多动作共用这一个打包器**（别再写第二个）：

- `extra_actions` 为空 → 只产**单视频循环**（idle-combo，loop=true）：呼吸/眨眼/摇尾
  焊死在同一支视频里，**没有 `idleSchedule`**，运行时纯循环播放。这条输出与多动作
  支持之前**逐字节相同**（黄金基线）。
- `extra_actions` 非空 → `actions` 多几条，`semantics` 按 `action_semantics()` 补键，
  `idleSchedule` 只收 `scheduled=True` 的那些（交互动作不进）。
  ⚠️ 动作的帧**必须已经复用 idle 的 crop box**（超框 = 硬失败），对齐是抠像那一步的事。

帧格式：`frame_format="png"`（默认，POC/内置基线）或 `"webp"`（产品，见 `encode_frame`）。
"""

from __future__ import annotations

import hashlib
import io
import json
import shutil
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from PIL import Image

FRAME_MS = 42

# 产品走 WebP：403MB -> 41MB，且 WebP 的 alpha 通道恒为无损（即使 lossless=False）。
# method=5 是「质量/耗时」的实测拐点；q90 与内置宠物 04/05 的现有资产一致。
FRAME_FORMATS = frozenset({"png", "webp"})
WEBP_QUALITY = 90
WEBP_METHOD = 5

# 产品会发出的全部动作（唯一真源：apps/desktop/src/runtime/pet-presentation-controller.ts）。
# schemaVersion-7 校验器要求 semantics 显式声明每一个键：没有专属动作的就显式指向
# defaultAction。键缺失与「故意不响应」在数据上不可区分，静默回落无法被验收。
PRODUCT_MOTIONS = (
    "idle",
    "look-left",
    "look-right",
    "react-happy",
    "react-curious",
    "carried",
    "landed",
    "sleep",
    "wake",
)

SPECIES = frozenset({"cat", "dog"})

# 交互动作（拎起）接管两个会用到「悬空保持」的产品动作。
INTERACTION_ACTION = "grab-release"
# 点身体 → 理毛（内置 04 的口径；老王的靶子就是「与内置 04 持平」）。
BODY_CLICK_MOTION = "react-curious"
BODY_CLICK_ACTION = "lick"


@dataclass(frozen=True)
class ExtraAction:
    """idle 之外的一支动作（偶发或交互），要并进**同一个**运行时包。

    ⚠️ 它的帧必须**已经对齐到 idle 的 crop box**（超框 = 硬失败）——
    本层只管打包，不做对齐（那是抠像那一步的事，见 `matting.matte_video(crop_box=...)`）。

    `hold_range` 是「悬空保持」的帧下标闭区间（只对交互动作有意义，且是**实测**出来的，
    见落地清单第七节第 5 片）；给不出就不写这个键 —— 别猜一个。
    `scheduled` 决定它进不进 `idleSchedule`（交互动作由 playMotion 触发，不进）。
    """

    action_id: str
    frames_dir: Path
    loop: bool = False
    hold_range: tuple[int, int] | None = None
    scheduled: bool = False
    min_interval_ms: int = 30_000
    max_interval_ms: int = 60_000


def action_semantics(action_ids: Sequence[str]) -> dict[str, str]:
    """多动作包的 `semantics`（唯一真源是内置 04/05 的那张表）。

    两条产品规则（**不是实现细节**，改它们等于改产品行为）：

    1. **每个动作自己有一个键**（`yawn` / `lick` / `grab-release`）——
       前端 `semantics[motion] ?? defaultAction` 靠它找到动作；键缺失就静默回落，
       无法验收。
    2. **`grab-release` 接管 `carried` / `landed`**（拖拽 → 拎起 → 松手），
       且 **`react-curious` → `lick`**（点身体 = 理毛）。
       没配这两个动作时它们照旧指向默认动作。

    ⚠️ 调用方负责保证 idle（defaultAction）自己不在这个列表里。
    """
    present = set(action_ids)
    overrides: dict[str, str] = {}
    if INTERACTION_ACTION in present:
        overrides["carried"] = INTERACTION_ACTION
        overrides["landed"] = INTERACTION_ACTION
    if BODY_CLICK_ACTION in present:
        overrides[BODY_CLICK_MOTION] = BODY_CLICK_ACTION
    return overrides


@dataclass(frozen=True)
class PackedFrameSequence:
    out_dir: Path
    manifest_path: Path
    manifest_sha256: str
    frame_count: int
    file_count: int
    total_bytes: int
    frame_duration_ms: int
    frame_format: str = "png"

    @property
    def duration_ms(self) -> int:
        return self.frame_duration_ms * self.frame_count


def encode_frame(png_bytes: bytes, frame_format: str, webp_quality: int = WEBP_QUALITY) -> bytes:
    """把源 PNG 的字节编码成目标帧格式。

    `"png"` 原样返回（不重新编码 —— 重编码会让黄金基线漂）。
    `"webp"` 用 `lossless=False, quality, method=5`：实测 403MB → 41MB，
    而 **WebP 的 alpha 通道恒为无损**（即便 lossless=False），所以透明边缘不会退化。
    """
    if frame_format == "png":
        return png_bytes
    if frame_format != "webp":
        raise ValueError(f"unsupported frame format: {frame_format}")
    if not 0 < webp_quality <= 100:
        raise ValueError(f"webpQuality must be in 1..100: {webp_quality}")
    with Image.open(io.BytesIO(png_bytes)) as image:
        rgba = image.convert("RGBA")
    buffer = io.BytesIO()
    rgba.save(buffer, "WEBP", lossless=False, quality=webp_quality, method=WEBP_METHOD)
    return buffer.getvalue()


def _require_safe_id(value: str, label: str) -> str:
    normalized = value.strip()
    if (
        not normalized
        or len(normalized) > 128
        or normalized in {".", ".."}
        or "/" in normalized
        or "\\" in normalized
    ):
        raise ValueError(f"invalid {label}: {value!r}")
    return normalized


def pack_frame_sequence(
    *,
    frames_dir: Path,
    out_dir: Path,
    pet_id: str,
    display_name: str,
    action_id: str = "idle-combo",
    species: str = "cat",
    frame_duration_ms: int = FRAME_MS,
    variant_id: str = "combo-loop-v1",
    frame_format: str = "png",
    webp_quality: int = WEBP_QUALITY,
    replace_existing: bool = False,
    extra_actions: Sequence[ExtraAction] = (),
) -> PackedFrameSequence:
    """把 `frames_dir` 下的 f*.png 打成 schema 7 包；`extra_actions` 是 idle 之外的动作。

    `extra_actions` 为空时输出**与多动作支持之前逐字节相同**（黄金基线）；
    非空时 `actions` 多几条、`semantics` 补上动作键、`idleSchedule` 按配置生成。

    `replace_existing` 必须显式给：目标目录非空时默认**拒绝**，
    因为原地重打包会先删掉旧帧（数百个文件），而本机的批量删除守卫会拦下来。

    `frame_format` 决定包里的帧用什么编码 + manifest 里写什么扩展名。
    默认 `"png"` 与搬入前逐字节一致（黄金基线）；产品走 `"webp"`。
    """

    frames_dir = Path(frames_dir)
    out_dir = Path(out_dir)
    if not frames_dir.is_dir():
        raise ValueError(f"frames directory does not exist: {frames_dir}")
    _require_safe_id(pet_id, "petId")
    _require_safe_id(action_id, "actionId")
    _require_safe_id(variant_id, "variantId")
    if species not in SPECIES:
        raise ValueError(f"unsupported species: {species}")
    if frame_format not in FRAME_FORMATS:
        raise ValueError(f"unsupported frame format: {frame_format}")
    if not display_name.strip():
        raise ValueError("displayName must be non-empty")
    if frame_duration_ms <= 0:
        raise ValueError("frameDurationMs must be positive")

    frame_paths = sorted(frames_dir.glob("f*.png"))
    if not frame_paths:
        raise ValueError(f"no f*.png frames in {frames_dir}")

    if out_dir.exists() and any(out_dir.iterdir()):
        if not replace_existing:
            raise ValueError(
                f"output directory is not empty (pass replace_existing to overwrite): {out_dir}"
            )
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    suffix = f".{frame_format}"
    files: list[dict[str, object]] = []

    def write_action(action_id_: str, paths: list[Path]) -> list[str]:
        """把一支动作的帧编码进包，返回相对路径（顺序 = 播放顺序）。"""
        rels: list[str] = []
        for index, source in enumerate(paths):
            relative = f"frames/{action_id_}/f{index:04d}{suffix}"
            destination = out_dir / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            data = encode_frame(source.read_bytes(), frame_format, webp_quality)
            destination.write_bytes(data)
            files.append(
                {
                    # idle 的 f0000 是静态兜底图（base）；其余（含动作的每一帧）都是 frame。
                    "role": "base" if action_id_ == action_id and index == 0 else "frame",
                    "relativePath": relative,
                    "sha256": hashlib.sha256(data).hexdigest(),
                }
            )
            rels.append(relative)
        return rels

    rel_frames = write_action(action_id, frame_paths)
    actions: list[dict[str, object]] = [
        {
            "actionId": action_id,
            "loop": True,
            "frameDurationMs": frame_duration_ms,
            "frames": rel_frames,
        }
    ]
    semantics = {motion: action_id for motion in PRODUCT_MOTIONS}
    idle_schedule: dict[str, object] | None = None

    if extra_actions:
        extras = list(extra_actions)
        seen = {action_id}
        for extra in extras:
            _require_safe_id(extra.action_id, "actionId")
            if extra.action_id in seen:
                raise ValueError(f"duplicate actionId in the package: {extra.action_id}")
            seen.add(extra.action_id)
        for extra in extras:
            extra_dir = Path(extra.frames_dir)
            if not extra_dir.is_dir():
                raise ValueError(f"frames directory does not exist: {extra_dir}")
            extra_paths = sorted(extra_dir.glob("f*.png"))
            if not extra_paths:
                raise ValueError(f"no f*.png frames in {extra_dir}")
            extra_rels = write_action(extra.action_id, extra_paths)
            entry: dict[str, object] = {
                "actionId": extra.action_id,
                "loop": extra.loop,
                "frameDurationMs": frame_duration_ms,
                "frames": extra_rels,
            }
            if extra.hold_range is not None:
                lo, hi = extra.hold_range
                if not 0 <= lo <= hi < len(extra_rels):
                    raise ValueError(
                        f"holdRange {extra.hold_range} is outside the {len(extra_rels)} frames "
                        f"of {extra.action_id}"
                    )
                entry["holdRange"] = [lo, hi]
            actions.append(entry)

        # 先套产品规则（carried/landed/react-curious），再让每个动作自己占一个键。
        semantics.update(action_semantics([extra.action_id for extra in extras]))
        for extra in extras:
            semantics[extra.action_id] = extra.action_id

        scheduled = [extra for extra in extras if extra.scheduled]
        if scheduled:
            idle_schedule = {
                "entries": [
                    {
                        "actionId": extra.action_id,
                        "weight": 1,
                        "minIntervalMs": extra.min_interval_ms,
                        "maxIntervalMs": extra.max_interval_ms,
                    }
                    for extra in scheduled
                ],
                # 一次性动作的帧复用 idle 第 0 帧的身体 → 必须对齐到循环边界，
                # 否则中途触发会把身体弹回起点（manifest 契约里写明这条）。
                "alignToDefaultLoop": True,
            }

    manifest: dict[str, object] = {
        "schemaVersion": 7,
        "renderer": "frame-sequence-v1",
        "petId": pet_id,
        "variantId": variant_id,
        "displayName": display_name,
        "species": species,
        "baseImage": f"frames/{action_id}/f0000{suffix}",
        "defaultAction": action_id,
        "anchorPolicy": "fixed",
        "actions": actions,
        "semantics": semantics,
    }
    if idle_schedule is not None:
        manifest["idleSchedule"] = idle_schedule
    manifest["files"] = files
    manifest["_provenance"] = {
        "generator": "photo_avatar_backend.frames.packing",
        "framesDir": str(frames_dir),
        "durationMs": frame_duration_ms * len(rel_frames),
        "note": (
            "单视频循环包：呼吸+眨眼+摇尾焊死在同一循环，无 idleSchedule，运行时纯循环播放。"
            if not extra_actions
            else f"多动作包：idle 一支循环 + {len(extra_actions)} 支动作"
            "（动作复用 idle 的 crop box，idleSchedule 只放偶发动作）。"
        ),
    }
    manifest_path = out_dir / "manifest.json"
    manifest_bytes = json.dumps(manifest, ensure_ascii=False, indent=2).encode("utf-8")
    manifest_path.write_bytes(manifest_bytes)

    total_bytes = sum(path.stat().st_size for path in out_dir.rglob("*") if path.is_file())
    return PackedFrameSequence(
        out_dir=out_dir,
        manifest_path=manifest_path,
        manifest_sha256=hashlib.sha256(manifest_bytes).hexdigest(),
        frame_count=len(rel_frames),
        file_count=len(files),
        total_bytes=total_bytes,
        frame_duration_ms=frame_duration_ms,
        frame_format=frame_format,
    )

"""把一帧序列目录打包成 frame-sequence-v1（schema 7）运行时包。

从 `scripts/poc_组合循环打包.py` 原样搬进服务（算法零改动，只加了入参校验与
「拒绝覆盖非空目录」的显式开关）。脚本现在只是这里的薄 CLI 包装。

只产**单视频循环**（idle-combo，loop=true）：呼吸/眨眼/摇尾焊死在同一支视频里，
所以 **没有 idleSchedule**，运行时纯循环播放。一次性动作（yawn/lick）另行接入，
它们必须复用 idle 的 crop box。

帧格式：`frame_format="png"`（默认，POC/内置基线）或 `"webp"`（产品，见 `encode_frame`）。
"""

from __future__ import annotations

import hashlib
import io
import json
import shutil
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
) -> PackedFrameSequence:
    """把 `frames_dir` 下的 f*.png 打成一个 schema 7 包。

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
    rel_frames: list[str] = []
    for index, source in enumerate(frame_paths):
        relative = f"frames/{action_id}/f{index:04d}{suffix}"
        destination = out_dir / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        data = encode_frame(source.read_bytes(), frame_format, webp_quality)
        destination.write_bytes(data)
        files.append(
            {
                "role": "base" if index == 0 else "frame",
                "relativePath": relative,
                "sha256": hashlib.sha256(data).hexdigest(),
            }
        )
        rel_frames.append(relative)

    manifest = {
        "schemaVersion": 7,
        "renderer": "frame-sequence-v1",
        "petId": pet_id,
        "variantId": variant_id,
        "displayName": display_name,
        "species": species,
        "baseImage": f"frames/{action_id}/f0000{suffix}",
        "defaultAction": action_id,
        "anchorPolicy": "fixed",
        "actions": [
            {
                "actionId": action_id,
                "loop": True,
                "frameDurationMs": frame_duration_ms,
                "frames": rel_frames,
            }
        ],
        "semantics": {motion: action_id for motion in PRODUCT_MOTIONS},
        "files": files,
        "_provenance": {
            "generator": "photo_avatar_backend.frames.packing",
            "framesDir": str(frames_dir),
            "durationMs": frame_duration_ms * len(rel_frames),
            "note": "单视频循环包：呼吸+眨眼+摇尾焊死在同一循环，无 idleSchedule，运行时纯循环播放。",
        },
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

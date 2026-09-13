"""把一帧序列目录打包成 frame-sequence-v1（schema 7）运行时包。

从 `scripts/poc_组合循环打包.py` 原样搬进服务（算法零改动，只加了入参校验与
「拒绝覆盖非空目录」的显式开关）。脚本现在只是这里的薄 CLI 包装。

只产**单视频循环**（idle-combo，loop=true）：呼吸/眨眼/摇尾焊死在同一支视频里，
所以 **没有 idleSchedule**，运行时纯循环播放。一次性动作（yawn/lick）另行接入，
它们必须复用 idle 的 crop box。
"""

from __future__ import annotations

import hashlib
import json
import shutil
from dataclasses import dataclass
from pathlib import Path

FRAME_MS = 42

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

    @property
    def duration_ms(self) -> int:
        return self.frame_duration_ms * self.frame_count


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


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
    replace_existing: bool = False,
) -> PackedFrameSequence:
    """把 `frames_dir` 下的 f*.png 打成一个 schema 7 包。

    `replace_existing` 必须显式给：目标目录非空时默认**拒绝**，
    因为原地重打包会先删掉旧帧（数百个文件），而本机的批量删除守卫会拦下来。
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

    files: list[dict[str, object]] = []
    rel_frames: list[str] = []
    for index, source in enumerate(frame_paths):
        relative = f"frames/{action_id}/f{index:04d}.png"
        destination = out_dir / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        files.append(
            {
                "role": "base" if index == 0 else "frame",
                "relativePath": relative,
                "sha256": sha256_of(source),
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
        "baseImage": f"frames/{action_id}/f0000.png",
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
    )

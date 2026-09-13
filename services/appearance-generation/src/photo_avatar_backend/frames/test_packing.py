"""打包器测试。

只验「文件搬运 + manifest 形状 + 拒绝覆盖」，不重复验 schema 7 的语义
（那是 `apps/desktop/src/runtime/frame-sequence-manifest.ts` 与 Rust
`runtime_assets/frame_sequence.rs` 的事）。帧内容不参与打包，所以用假 PNG 即可。
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from photo_avatar_backend.frames.packing import (
    FRAME_MS,
    PRODUCT_MOTIONS,
    pack_frame_sequence,
)


def write_frames(root: Path, count: int) -> Path:
    frames = root / "frames"
    frames.mkdir(parents=True)
    for index in range(count):
        (frames / f"f{index:04d}.png").write_bytes(b"\x89PNG\r\n\x1a\n" + bytes([index]) * 8)
    return frames


def pack(frames: Path, out: Path, **overrides):
    kwargs = {
        "frames_dir": frames,
        "out_dir": out,
        "pet_id": "06-guodong",
        "display_name": "果冻（短毛猫）",
    }
    kwargs.update(overrides)
    return pack_frame_sequence(**kwargs)


def test_manifest_matches_schema_seven_shape(tmp_path: Path):
    frames = write_frames(tmp_path, 3)

    packed = pack(frames, tmp_path / "out")
    manifest = json.loads(packed.manifest_path.read_text(encoding="utf-8"))

    assert manifest["schemaVersion"] == 7
    assert manifest["renderer"] == "frame-sequence-v1"
    assert manifest["petId"] == "06-guodong"
    assert manifest["displayName"] == "果冻（短毛猫）"
    assert manifest["species"] == "cat"
    assert manifest["baseImage"] == "frames/idle-combo/f0000.png"
    assert manifest["defaultAction"] == "idle-combo"
    assert manifest["anchorPolicy"] == "fixed"
    assert manifest["actions"] == [
        {
            "actionId": "idle-combo",
            "loop": True,
            "frameDurationMs": FRAME_MS,
            "frames": [
                "frames/idle-combo/f0000.png",
                "frames/idle-combo/f0001.png",
                "frames/idle-combo/f0002.png",
            ],
        }
    ]
    # semantics 必须显式声明产品动作全集，且这些键一个都不能少
    assert set(manifest["semantics"]) == set(PRODUCT_MOTIONS)
    assert set(manifest["semantics"].values()) == {"idle-combo"}
    # 单视频循环包不应有 idleSchedule
    assert "idleSchedule" not in manifest
    assert packed.frame_count == 3
    assert packed.duration_ms == 3 * FRAME_MS


def test_copied_frames_and_hashes_match_the_source(tmp_path: Path):
    frames = write_frames(tmp_path, 4)

    packed = pack(frames, tmp_path / "out")
    manifest = json.loads(packed.manifest_path.read_text(encoding="utf-8"))

    assert [entry["role"] for entry in manifest["files"]] == ["base", "frame", "frame", "frame"]
    for entry in manifest["files"]:
        copied = packed.out_dir / entry["relativePath"]
        assert copied.read_bytes() == (frames / Path(entry["relativePath"]).name).read_bytes()
        assert hashlib.sha256(copied.read_bytes()).hexdigest() == entry["sha256"]


def test_frame_order_follows_source_index(tmp_path: Path):
    """产物会按序**重命名**为 f0000…，所以「顺序对不对」只能看内容。

    源文件名带 4 位零填充，字典序与数值序一致；但目标名是重新编号的，
    拿目标名断言顺序等于什么都没验。
    """
    frames = tmp_path / "frames"
    frames.mkdir()
    for index in (0, 9, 10, 100, 1000):
        (frames / f"f{index:04d}.png").write_bytes(b"\x89PNG\r\n\x1a\n" + str(index).encode())

    packed = pack(frames, tmp_path / "out")
    manifest = json.loads(packed.manifest_path.read_text(encoding="utf-8"))
    manifest_frames = manifest["actions"][0]["frames"]

    assert [Path(item).name for item in manifest_frames] == [
        "f0000.png", "f0001.png", "f0002.png", "f0003.png", "f0004.png",
    ]
    contents = [
        (packed.out_dir / item).read_bytes().removeprefix(b"\x89PNG\r\n\x1a\n").decode()
        for item in manifest_frames
    ]
    assert contents == ["0", "9", "10", "100", "1000"]
    assert packed.frame_count == 5


def test_refuses_non_empty_output_directory(tmp_path: Path):
    frames = write_frames(tmp_path, 2)
    out = tmp_path / "out"
    out.mkdir()
    (out / "stale.txt").write_text("旧产物", encoding="utf-8")

    with pytest.raises(ValueError, match="not empty"):
        pack(frames, out)

    assert (out / "stale.txt").exists()


def test_replaces_non_empty_output_directory_when_asked(tmp_path: Path):
    frames = write_frames(tmp_path, 2)
    out = tmp_path / "out"
    out.mkdir()
    (out / "stale.txt").write_text("旧产物", encoding="utf-8")

    packed = pack(frames, out, replace_existing=True)

    assert not (out / "stale.txt").exists()
    assert packed.manifest_path.exists()


@pytest.mark.parametrize(
    ("overrides", "match"),
    [
        ({"pet_id": "../escape"}, "invalid petId"),
        ({"pet_id": ""}, "invalid petId"),
        ({"action_id": "a/b"}, "invalid actionId"),
        ({"species": "rabbit"}, "unsupported species"),
        ({"display_name": "   "}, "displayName"),
        ({"frame_duration_ms": 0}, "frameDurationMs"),
    ],
)
def test_rejects_invalid_arguments(tmp_path: Path, overrides, match):
    frames = write_frames(tmp_path, 1)

    with pytest.raises(ValueError, match=match):
        pack(frames, tmp_path / "out", **overrides)


def test_rejects_missing_or_empty_frames_directory(tmp_path: Path):
    with pytest.raises(ValueError, match="does not exist"):
        pack(tmp_path / "nope", tmp_path / "out")

    empty = tmp_path / "empty"
    empty.mkdir()
    with pytest.raises(ValueError, match="no f\\*.png"):
        pack(empty, tmp_path / "out")


def test_species_is_written_through_for_dogs(tmp_path: Path):
    """schema 7 的 manifest 里 species 是真字段 —— 上传狗不能被记成猫。"""
    frames = write_frames(tmp_path, 1)

    packed = pack(frames, tmp_path / "out", pet_id="08-duanmaoquan", species="dog")
    manifest = json.loads(packed.manifest_path.read_text(encoding="utf-8"))

    assert manifest["species"] == "dog"

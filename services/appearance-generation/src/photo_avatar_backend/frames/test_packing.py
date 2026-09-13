"""打包器测试。

只验「文件搬运 + manifest 形状 + 拒绝覆盖」，不重复验 schema 7 的语义
（那是 `apps/desktop/src/runtime/frame-sequence-manifest.ts` 与 Rust
`runtime_assets/frame_sequence.rs` 的事）。帧内容不参与打包，所以用假 PNG 即可。
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np
import pytest
from PIL import Image

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


# ------------------------------------------------------------------ WebP 路径

def write_real_frames(root: Path, count: int, size: int = 64) -> Path:
    """写**真**PNG（有渐变、有透明边），让 WebP 编码器有东西可压。"""
    frames = root / "frames"
    frames.mkdir(parents=True)
    yy, xx = np.mgrid[0:size, 0:size]
    for index in range(count):
        rgba = np.zeros((size, size, 4), np.uint8)
        rgba[:, :, 0] = (xx * 3 + index) % 256
        rgba[:, :, 1] = (yy * 5) % 256
        rgba[:, :, 2] = ((xx + yy) * 2) % 256
        rgba[:, :, 3] = np.where((xx > 4) & (xx < size - 4) & (yy > 4) & (yy < size - 4), 255, 0)
        Image.fromarray(rgba).save(frames / f"f{index:04d}.png")
    return frames


def test_webp_pack_encodes_frames_and_rewrites_the_manifest(tmp_path: Path):
    frames = write_real_frames(tmp_path, 3)

    packed = pack(frames, tmp_path / "out", frame_format="webp")
    manifest = json.loads(packed.manifest_path.read_text(encoding="utf-8"))

    assert packed.frame_format == "webp"
    assert manifest["baseImage"] == "frames/idle-combo/f0000.webp"
    assert [Path(item).suffix for item in manifest["actions"][0]["frames"]] == [".webp"] * 3
    assert [Path(entry["relativePath"]).suffix for entry in manifest["files"]] == [".webp"] * 3
    assert not list(packed.out_dir.rglob("*.png")), "包里不该留 PNG"
    for entry in manifest["files"]:
        blob = (packed.out_dir / entry["relativePath"]).read_bytes()
        assert blob[:4] == b"RIFF" and blob[8:12] == b"WEBP"
        assert hashlib.sha256(blob).hexdigest() == entry["sha256"]


def test_webp_keeps_alpha_exactly(tmp_path: Path):
    """WebP 的 alpha 通道恒为无损（即便 lossless=False）—— 透明边缘不能退化。"""
    frames = write_real_frames(tmp_path, 1)

    packed = pack(frames, tmp_path / "out", frame_format="webp")
    source = np.array(Image.open(frames / "f0000.png").convert("RGBA"))
    decoded = np.array(
        Image.open(packed.out_dir / "frames" / "idle-combo" / "f0000.webp").convert("RGBA")
    )

    assert (decoded[:, :, 3] == source[:, :, 3]).all(), "alpha 必须逐像素相同"


def test_webp_is_much_smaller_than_png(tmp_path: Path):
    """**合成图不能用来断言「WebP 更小」** —— 这条测试存在的意义是把这个坑钉住。

    最初这里写的是 `assert webps * 2 < pngs`，实测直接翻车：
    规律渐变图的 PNG 只要 2784 字节，同内容 WebP(q90) 却要 7052 ——
    **lossy 编码器把码率花在人眼关心的细节上，碰上 PNG 的送分题（可预测条纹）自然输**。
    体积结论只能在**真实帧**上成立：`output/一键出宠-测试-2026-09-13/` 那只
    614×614 × 288 帧，PNG 源帧 72.6 MB → WebP 包 9.7 MB（约 7.5 倍）。
    所以这里只验「两种格式都能出合法包」，体积交给真实产物的回归记录。
    """
    frames = write_real_frames(tmp_path, 2, size=128)

    png_pack = pack(frames, tmp_path / "out-png")
    webp_pack = pack(frames, tmp_path / "out-webp", frame_format="webp")

    assert png_pack.frame_count == webp_pack.frame_count == 2
    assert png_pack.frame_format == "png"
    assert webp_pack.frame_format == "webp"
    manifest = json.loads(webp_pack.manifest_path.read_text(encoding="utf-8"))
    assert all(Path(item).suffix == ".webp" for item in manifest["actions"][0]["frames"])


def test_png_pack_uses_source_bytes_verbatim(tmp_path: Path):
    """PNG 路径**不重新编码** —— 重编码会让黄金基线漂。"""
    frames = write_real_frames(tmp_path, 2)

    packed = pack(frames, tmp_path / "out")

    for index in range(2):
        assert (
            packed.out_dir / "frames" / "idle-combo" / f"f{index:04d}.png"
        ).read_bytes() == (frames / f"f{index:04d}.png").read_bytes()


@pytest.mark.parametrize(
    ("overrides", "match"),
    [
        ({"frame_format": "avif"}, "unsupported frame format"),
        ({"frame_format": "webp", "webp_quality": 0}, "webpQuality"),
        ({"frame_format": "webp", "webp_quality": 101}, "webpQuality"),
    ],
)
def test_rejects_invalid_frame_format(tmp_path: Path, overrides, match):
    frames = write_frames(tmp_path, 1)

    with pytest.raises(ValueError, match=match):
        pack(frames, tmp_path / "out", **overrides)

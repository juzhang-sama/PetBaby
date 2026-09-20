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
    ExtraAction,
    map_to_planned_index,
    pack_frame_sequence,
    plan_frame_indices,
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


# ------------------------------------------------------------------ 多动作包
#
# idle 之外的动作各是一支视频、各自抠像，但**并进同一个运行时包**
# （一个 job 一个 artifact → 最终包必须是一支 zip）。


def _extra(tmp_path: Path, action_id: str, count: int = 4, **overrides) -> ExtraAction:
    frames = write_frames(tmp_path / f"extra-{action_id}", count)
    kwargs: dict = {"action_id": action_id, "frames_dir": frames}
    kwargs.update(overrides)
    return ExtraAction(**kwargs)


def test_a_multi_action_package_declares_every_action_and_schedules_the_asked_ones(
    tmp_path: Path,
):
    idle = write_frames(tmp_path, 3)
    yawn = _extra(tmp_path, "yawn", scheduled=True)
    lick = _extra(tmp_path, "lick", scheduled=True)
    grab = _extra(tmp_path, "grab-release", hold_range=(1, 2))

    packed = pack(idle, tmp_path / "out", extra_actions=[yawn, lick, grab])
    manifest = json.loads(packed.manifest_path.read_text(encoding="utf-8"))

    assert [action["actionId"] for action in manifest["actions"]] == [
        "idle-combo", "yawn", "lick", "grab-release",
    ]
    # 每条动作的帧都进了包，且 files 里逐条登记（Rust 安装时会逐文件校 sha256）
    declared = {entry["relativePath"] for entry in manifest["files"]}
    for action in manifest["actions"]:
        for frame in action["frames"]:
            assert frame in declared
    assert len(manifest["files"]) == 3 + 4 * 3
    # 动作的帧都是 frame；base 只有 idle 的 f0000（静态兜底图）
    yawn_roles = [
        entry["role"]
        for entry in manifest["files"]
        if entry["relativePath"].startswith("frames/yawn/")
    ]
    assert yawn_roles == ["frame"] * 4
    assert manifest["baseImage"] == "frames/idle-combo/f0000.png"

    # semantics：9 个产品 motion 一个都不能少，且两条产品规则生效
    assert set(PRODUCT_MOTIONS) <= set(manifest["semantics"])
    assert manifest["semantics"]["yawn"] == "yawn"
    assert manifest["semantics"]["lick"] == "lick"
    assert manifest["semantics"]["react-curious"] == "lick"       # 点身体 = 理毛
    assert manifest["semantics"]["carried"] == "grab-release"     # 拖拽 = 拎起
    assert manifest["semantics"]["landed"] == "grab-release"
    assert manifest["semantics"]["idle"] == "idle-combo"          # 没被动作顶掉

    # idleSchedule 只收 scheduled=True 的那两支；交互动作不进（它由 playMotion 触发）
    schedule = manifest["idleSchedule"]
    assert [entry["actionId"] for entry in schedule["entries"]] == ["yawn", "lick"]
    assert schedule["alignToDefaultLoop"] is True
    assert all(entry["weight"] == 1 for entry in schedule["entries"])


def test_the_interaction_action_carries_its_hold_range(tmp_path: Path):
    idle = write_frames(tmp_path, 2)
    grab = _extra(tmp_path, "grab-release", count=10, hold_range=(3, 7))

    manifest = json.loads(
        pack(idle, tmp_path / "out", extra_actions=[grab]).manifest_path.read_text(
            encoding="utf-8"
        )
    )

    assert manifest["actions"][1]["holdRange"] == [3, 7]


def test_a_hold_range_outside_the_action_frames_is_rejected(tmp_path: Path):
    idle = write_frames(tmp_path, 2)
    grab = _extra(tmp_path, "grab-release", count=4, hold_range=(0, 4))  # hi 越界

    with pytest.raises(ValueError, match="holdRange"):
        pack(idle, tmp_path / "out", extra_actions=[grab])


# ------------------------------------------------------------------ 精修（裁 + 重采样）
#
# 产线视频固定 12 秒、动作摊在 3 倍时长里还带几秒静止废料，内置宠是人工裁过节奏的
# （建国 grab-release = 81 帧 3.4s，新宠原生 = 287 帧 12.1s）。
# `frameRange` 裁废料、`frameTarget` 均匀重采样到内置宠量级 —— **0 算力**，
# 见 `docs/设计/动作精修流程-2026-09-20.md`。


def test_no_refinement_keys_takes_every_frame():
    assert plan_frame_indices(5) == [0, 1, 2, 3, 4]


def test_a_frame_range_only_crops_the_tail():
    assert plan_frame_indices(10, (2, 6)) == [2, 3, 4, 5, 6]


def test_a_frame_target_only_resamples_the_whole_clip():
    """均匀重采样**含首含尾**：尾巴不能丢，否则收回动作少最后一步。"""
    assert plan_frame_indices(10, None, 4) == [0, 3, 6, 9]


def test_a_frame_range_and_a_frame_target_compose():
    assert plan_frame_indices(10, (0, 6), 3) == [0, 3, 6]


@pytest.mark.parametrize(
    "total, frame_range, frame_target, match",
    [
        (10, (0, 10), None, "frameRange"),      # hi 越界
        (10, (5, 2), None, "frameRange"),       # 反序
        (10, None, 1, "frameTarget"),           # 少于 2 帧
        (10, (0, 3), 8, "frameTarget"),         # 目标比素材还多 = 复制帧，不是精修
    ],
)
def test_a_bad_refinement_plan_is_rejected(total, frame_range, frame_target, match):
    with pytest.raises(ValueError, match=match):
        plan_frame_indices(total, frame_range, frame_target)


def test_mapping_a_source_index_reuses_the_plan_itself():
    """映射必须复用同一份 plan —— 两个方向各算一遍逆公式迟早会漂一帧。"""
    plan = plan_frame_indices(10, (0, 6), 3)   # [0, 3, 6]

    assert map_to_planned_index(0, plan) == 0
    assert map_to_planned_index(3, plan) == 1
    assert map_to_planned_index(6, plan) == 2
    assert map_to_planned_index(4, plan) == 1  # 最近邻，落在 3 上


def test_refinement_rewrites_the_hold_range_into_packed_coordinates(tmp_path: Path):
    idle = write_frames(tmp_path, 2)
    grab = _extra(
        tmp_path, "grab-release", count=9, hold_range=(3, 6), frame_target=4
    )

    manifest = json.loads(
        pack(idle, tmp_path / "out", extra_actions=[grab]).manifest_path.read_text(
            encoding="utf-8"
        )
    )
    action = manifest["actions"][1]

    # plan = [0, 3, 5, 8] ⇒ 原始 [3, 6] 最近邻映射到 [1, 2]
    assert len(action["frames"]) == 4
    assert action["frames"] == [f"frames/grab-release/f{index:04d}.png" for index in range(4)]
    assert action["holdRange"] == [1, 2]
    # 包内只登记留下的那 4 帧（Rust 安装时逐文件校 sha256）
    assert len([entry for entry in manifest["files"] if "grab-release" in entry["relativePath"]]) == 4


def test_a_hold_range_that_collapses_under_resampling_is_rejected(tmp_path: Path):
    """窗口比采样间隔还短 ⇒ 映射后 lo == hi = 一张静止图，必须当场报错而不是静默出包。"""
    idle = write_frames(tmp_path, 2)
    grab = _extra(
        tmp_path, "grab-release", count=100, hold_range=(50, 51), frame_target=4
    )

    with pytest.raises(ValueError, match="collapses"):
        pack(idle, tmp_path / "out", extra_actions=[grab])


def test_refinement_logs_the_numbers_it_used(tmp_path: Path):
    """精修是人定的数字，映射结果必须看得见（看不见就等于黑箱）。"""
    lines: list[str] = []
    idle = write_frames(tmp_path, 2)
    grab = _extra(tmp_path, "grab-release", count=10, hold_range=(2, 7), frame_target=4)

    pack(idle, tmp_path / "out", extra_actions=[grab], log=lines.append)
    text = "\n".join(lines)

    assert "原始 10 帧" in text
    assert "[精修] grab-release：holdRange [2, 7]（原始帧）→ [" in text


def test_a_single_action_package_still_has_no_idle_schedule(tmp_path: Path):
    """多动作支持**不许**改变单动作的输出（黄金基线）。"""
    manifest = json.loads(
        pack(write_frames(tmp_path, 3), tmp_path / "out").manifest_path.read_text(
            encoding="utf-8"
        )
    )

    assert "idleSchedule" not in manifest
    assert len(manifest["actions"]) == 1
    assert manifest["semantics"] == {
        motion: "idle-combo" for motion in PRODUCT_MOTIONS
    }


def test_an_action_repeating_another_action_id_is_rejected(tmp_path: Path):
    idle = write_frames(tmp_path, 2)

    with pytest.raises(ValueError, match="duplicate actionId"):
        pack(
            idle,
            tmp_path / "out",
            extra_actions=[ExtraAction(action_id="idle-combo", frames_dir=idle)],
        )

    yawn = _extra(tmp_path, "yawn")
    with pytest.raises(ValueError, match="duplicate actionId"):
        pack(idle, tmp_path / "out2", extra_actions=[yawn, yawn])


# ------------------------------------------------------------------ 跨语言契约 fixture
#
# 一份文件两边共用：这里断言「我们**现在**打出来的就是它」（防打包器悄悄漂移），
# Rust 侧（frame_sequence.rs）断言「它进得来」（防校验器比前端更严）。

CONTRACT_FIXTURE = (
    Path(__file__).resolve().parents[5]
    / "apps/desktop/src-tauri/tests/fixtures/photo-avatar/frame-sequence-v7-multi-action.json"
)


def test_the_shared_contract_fixture_is_still_exactly_what_we_produce(tmp_path: Path):
    def frames(name: str, count: int) -> Path:
        out = tmp_path / name / "frames"
        out.mkdir(parents=True)
        for index in range(count):
            (out / f"f{index:04d}.png").write_bytes(
                b"\x89PNG\r\n\x1a\n" + bytes([index]) * 8
            )
        return out

    packed = pack_frame_sequence(
        frames_dir=frames("idle", 4),
        out_dir=tmp_path / "package",
        pet_id="99-contract",
        display_name="契约样例（短毛猫）",
        variant_id="photo-avatar-session-contract-1",
        frame_format="png",
        extra_actions=[
            ExtraAction(action_id="yawn", frames_dir=frames("yawn", 3), scheduled=True),
            ExtraAction(action_id="lick", frames_dir=frames("lick", 3), scheduled=True),
            ExtraAction(
                action_id="grab-release",
                frames_dir=frames("grab-release", 5),
                hold_range=(1, 3),
            ),
        ],
    )

    produced = json.loads(packed.manifest_path.read_text(encoding="utf-8"))
    fixture = json.loads(CONTRACT_FIXTURE.read_text(encoding="utf-8"))
    # _provenance.framesDir 记的是「在哪台机器、哪个临时目录跑的」→ 天然逐机不同。
    produced.pop("_provenance")
    fixture.pop("_provenance")

    assert produced == fixture, (
        "打包器的输出形状变了。这份 fixture 是 Python 与 Rust 共用的契约 —— "
        "改它之前先看 apps/desktop/src-tauri/src/runtime_assets/frame_sequence.rs "
        "的 parses_the_shared_multi_action_contract_fixture 还认不认"
    )

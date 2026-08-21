from __future__ import annotations

import hashlib
from io import BytesIO
import json
from pathlib import Path
import shutil

import pytest
from PIL import Image

from .semantic_layers import SEMANTIC_LAYER_IDS, build_semantic_layer_specs
from .uv_guides import (
    build_work_canvas,
    build_module_semantic_snapshot,
    generate_guides,
)


BODY_MODULE_IDS = (
    "body-slender-v1",
    "body-balanced-v1",
    "body-rounded-v1",
)
MODULE_ROOT = (
    Path(__file__).resolve().parents[4]
    / "apps"
    / "desktop"
    / "public"
    / "cat-character-modules"
    / "cat-a-live2d-v1"
)


def _copy_modules(target_root: Path) -> None:
    for module_id in BODY_MODULE_IDS:
        source_dir = MODULE_ROOT / module_id
        target_dir = target_root / module_id
        target_dir.mkdir(parents=True)
        shutil.copy2(source_dir / "模块.json", target_dir / "模块.json")
        neutral_dir = target_dir / f"{module_id}.2048"
        neutral_dir.mkdir()
        shutil.copy2(
            source_dir / f"{module_id}.2048" / "texture_00.png",
            neutral_dir / "texture_00.png",
        )


def _neutral_png(module_id: str) -> bytes:
    module_dir = MODULE_ROOT / module_id
    return (module_dir / f"{module_id}.2048" / "texture_00.png").read_bytes()


def _open_rgba(png: bytes) -> Image.Image:
    with Image.open(BytesIO(png)) as image:
        return image.convert("RGBA")


def test_work_canvas_does_not_read_neutral_rgb():
    neutral = _open_rgba(_neutral_png("body-balanced-v1"))
    replacement = Image.new("RGBA", neutral.size, (231, 17, 193, 0))
    replacement.putalpha(neutral.getchannel("A"))
    buffer = BytesIO()
    replacement.save(buffer, format="PNG")

    assert build_work_canvas(buffer.getvalue()) == build_work_canvas(
        _neutral_png("body-balanced-v1")
    )


@pytest.mark.parametrize("module_id", BODY_MODULE_IDS)
def test_module_semantic_snapshot_binds_all_masks_to_the_module_contract(
    module_id: str,
):
    module_dir = MODULE_ROOT / module_id
    contract = (module_dir / "模块.json").read_bytes()
    snapshot = build_module_semantic_snapshot(
        module_id,
        contract,
        _neutral_png(module_id),
    )
    specs = build_semantic_layer_specs(snapshot)

    assert tuple(spec.layer_id for spec in specs) == SEMANTIC_LAYER_IDS
    assert all(spec.mask_sha256 == snapshot.layer_mask_sha256[spec.layer_id] for spec in specs)
    assert all(
        (spec.width, spec.height) == (snapshot.width, snapshot.height)
        for spec in specs
    )


@pytest.mark.parametrize("module_id", BODY_MODULE_IDS)
def test_work_canvas_is_fully_opaque_and_region_map_matches_source_alpha(
    module_id: str,
):
    neutral_png = _neutral_png(module_id)
    bundle = build_work_canvas(neutral_png)
    work = _open_rgba(bundle.work_canvas_png)
    with Image.open(BytesIO(bundle.region_map_png)) as image:
        regions = image.copy()
    source = _open_rgba(neutral_png)

    assert set(work.getchannel("A").get_flattened_data()) == {255}
    assert regions.mode == "L"
    assert all(
        (region > 0) == (alpha > 0)
        for region, alpha in zip(
            regions.get_flattened_data(),
            source.getchannel("A").get_flattened_data(),
            strict=True,
        )
    )
    assert hashlib.sha256(bundle.source_alpha).hexdigest() == bundle.source_alpha_sha256


def test_generation_records_work_canvas_region_map_and_source_hashes(tmp_path: Path):
    target_root = (
        tmp_path
        / "apps"
        / "desktop"
        / "public"
        / "cat-character-modules"
        / "cat-a-live2d-v1"
    )
    _copy_modules(target_root)

    index = generate_guides(tmp_path)
    output_root = (
        tmp_path
        / "services"
        / "appearance-generation"
        / "src"
        / "photo_avatar_backend"
        / "assets"
        / "uv-guides"
    )

    for entry in index["guides"]:
        module_id = entry["moduleId"]
        expected_paths = {
            "workCanvasPath": f"{module_id}.work.png",
            "regionMapPath": f"{module_id}.regions.png",
        }
        for field, relative in expected_paths.items():
            assert entry[field] == relative
            assert (output_root / relative).resolve().parent == output_root.resolve()
        work_sha256 = hashlib.sha256(
            (output_root / entry["workCanvasPath"]).read_bytes()
        ).hexdigest()
        region_sha256 = hashlib.sha256(
            (output_root / entry["regionMapPath"]).read_bytes()
        ).hexdigest()
        assert work_sha256 == entry["workCanvasSha256"]
        assert region_sha256 == entry["regionMapSha256"]

        module_dir = target_root / module_id
        contract = json.loads((module_dir / "模块.json").read_text(encoding="utf-8"))
        source_bytes = (module_dir / contract["files"]["neutralTexture"]).read_bytes()
        assert hashlib.sha256(source_bytes).hexdigest() == entry["sourceTextureSha256"]
        with Image.open(BytesIO(source_bytes)) as source:
            alpha = source.getchannel("A").tobytes()
        assert hashlib.sha256(alpha).hexdigest() == entry["sourceAlphaSha256"]


@pytest.mark.parametrize(
    "image",
    [
        Image.new("RGBA", (1024, 2048), (0, 0, 0, 0)),
        Image.new("RGB", (2048, 2048), (0, 0, 0)),
    ],
)
def test_work_canvas_rejects_wrong_size_or_missing_alpha(image: Image.Image):
    buffer = BytesIO()
    image.save(buffer, format="PNG")

    with pytest.raises(ValueError):
        build_work_canvas(buffer.getvalue())


def test_generation_writes_schema_v2_without_legacy_guides(tmp_path: Path):
    target_root = (
        tmp_path
        / "apps"
        / "desktop"
        / "public"
        / "cat-character-modules"
        / "cat-a-live2d-v1"
    )
    _copy_modules(target_root)
    output_root = (
        tmp_path
        / "services"
        / "appearance-generation"
        / "src"
        / "photo_avatar_backend"
        / "assets"
        / "uv-guides"
    )
    output_root.mkdir(parents=True)
    for module_id in BODY_MODULE_IDS:
        (output_root / f"{module_id}.png").write_bytes(b"legacy")

    index = generate_guides(tmp_path)

    assert index["schemaVersion"] == 2
    for entry in index["guides"]:
        assert "relativePath" not in entry
        assert "guideSha256" not in entry
        assert not (output_root / f"{entry['moduleId']}.png").exists()


def test_generation_preserves_reviewed_lf_index_byte_for_byte(tmp_path: Path):
    target_root = (
        tmp_path
        / "apps"
        / "desktop"
        / "public"
        / "cat-character-modules"
        / "cat-a-live2d-v1"
    )
    _copy_modules(target_root)
    index = generate_guides(tmp_path)
    for entry in index["guides"]:
        entry["visualReview"] = {
            "status": "passed",
            "tool": "view_image",
            "conclusion": "reviewed",
        }
    index_path = (
        tmp_path
        / "services"
        / "appearance-generation"
        / "src"
        / "photo_avatar_backend"
        / "assets"
        / "uv-guides"
        / "索引.json"
    )
    index_path.write_text(
        json.dumps(index, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    before = index_path.read_bytes()

    generate_guides(tmp_path)

    assert index_path.read_bytes() == before


@pytest.mark.parametrize("path_kind", ("traversal", "absolute"))
def test_generation_rejects_neutral_texture_outside_module_directory(
    tmp_path: Path, path_kind: str
):
    target_root = (
        tmp_path
        / "apps"
        / "desktop"
        / "public"
        / "cat-character-modules"
        / "cat-a-live2d-v1"
    )
    _copy_modules(target_root)
    module_dir = target_root / "body-balanced-v1"
    outside = (
        target_root / "escaped.png"
        if path_kind == "traversal"
        else tmp_path / "absolute.png"
    )
    outside.write_bytes(_neutral_png("body-balanced-v1"))
    contract_path = module_dir / "模块.json"
    contract = json.loads(contract_path.read_text(encoding="utf-8"))
    contract["files"]["neutralTexture"] = (
        "../escaped.png" if path_kind == "traversal" else str(outside.resolve())
    )
    contract_path.write_text(json.dumps(contract), encoding="utf-8")

    with pytest.raises(ValueError, match="module directory"):
        generate_guides(tmp_path)


@pytest.mark.parametrize(
    "changed_field",
    (
        "sourceAlphaSha256",
        "workCanvasSha256",
        "regionMapSha256",
        "sourceTextureSha256",
    ),
)
def test_generation_invalidates_visual_review_when_bound_hash_changes(
    tmp_path: Path, changed_field: str
):
    target_root = (
        tmp_path
        / "apps"
        / "desktop"
        / "public"
        / "cat-character-modules"
        / "cat-a-live2d-v1"
    )
    _copy_modules(target_root)
    index = generate_guides(tmp_path)
    for entry in index["guides"]:
        entry["visualReview"] = {
            "status": "passed",
            "tool": "view_image",
            "conclusion": "reviewed",
        }
    balanced = next(
        entry for entry in index["guides"] if entry["moduleId"] == "body-balanced-v1"
    )
    balanced[changed_field] = "0" * 64
    index_path = (
        tmp_path
        / "services"
        / "appearance-generation"
        / "src"
        / "photo_avatar_backend"
        / "assets"
        / "uv-guides"
        / "索引.json"
    )
    index_path.write_text(json.dumps(index), encoding="utf-8")

    regenerated = generate_guides(tmp_path)

    reviews = {
        entry["moduleId"]: entry["visualReview"] for entry in regenerated["guides"]
    }
    assert reviews["body-balanced-v1"]["status"] == "pending"
    assert reviews["body-slender-v1"]["status"] == "passed"
    assert reviews["body-rounded-v1"]["status"] == "passed"

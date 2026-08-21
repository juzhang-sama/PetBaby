"""Build deterministic identity-neutral UV guides from module alpha masks."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
from io import BytesIO
import json
from pathlib import Path
from typing import Any, Mapping

from PIL import Image

from .semantic_layers import ModuleSemanticSnapshot, SEMANTIC_LAYER_IDS


GENERATOR_VERSION = "opaque-uv-work-canvas-v2"
BACKGROUND_COLOR = (17, 19, 23, 255)
BODY_MODULE_IDS = (
    "body-slender-v1",
    "body-balanced-v1",
    "body-rounded-v1",
)
ALLOWED_GUIDE_COLORS = frozenset(
    {
        (214, 72, 72),
        (47, 138, 151),
        (236, 190, 72),
        (92, 149, 84),
        (106, 91, 157),
        (218, 112, 63),
    }
)
_PALETTE = tuple(sorted(ALLOWED_GUIDE_COLORS))
_PNG_SIZE = (2048, 2048)


@dataclass(frozen=True)
class WorkCanvasBundle:
    work_canvas_png: bytes
    region_map_png: bytes
    source_alpha: bytes
    source_alpha_sha256: str


def resolve_module_file(module_dir: Path, relative: str) -> Path:
    """Resolve a contract file while keeping it inside its module directory."""

    root = module_dir.resolve()
    candidate = (root / relative).resolve()
    try:
        candidate.relative_to(root)
    except ValueError as exc:
        raise ValueError("module file must remain inside module directory") from exc
    return candidate


def build_work_canvas(neutral_png: bytes) -> WorkCanvasBundle:
    """Build an opaque provider canvas and its source-alpha region map."""

    source = _decode_neutral(neutral_png)
    alpha = source.getchannel("A")
    work = Image.new("RGBA", source.size, BACKGROUND_COLOR)
    regions = Image.new("L", source.size, 0)
    work_pixels = work.load()
    region_pixels = regions.load()
    alpha_pixels = alpha.load()
    for y in range(source.height):
        row = min(2, (y * 3) // source.height)
        for x in range(source.width):
            if alpha_pixels[x, y] > 0:
                column = min(1, (x * 2) // source.width)
                region_id = row * 2 + column + 1
                work_pixels[x, y] = (*_PALETTE[region_id - 1], 255)
                region_pixels[x, y] = region_id
    source_alpha = alpha.tobytes()
    return WorkCanvasBundle(
        work_canvas_png=_encode_png(work),
        region_map_png=_encode_png(regions),
        source_alpha=source_alpha,
        source_alpha_sha256=hashlib.sha256(source_alpha).hexdigest(),
    )


def build_module_semantic_snapshot(
    body_module_id: str,
    module_contract: bytes,
    neutral_png: bytes,
) -> ModuleSemanticSnapshot:
    contract = json.loads(module_contract.decode("utf-8"))
    if contract.get("moduleId") != body_module_id:
        raise ValueError("semantic snapshot module ID does not match contract")
    source = _decode_neutral(neutral_png)
    source_alpha = source.getchannel("A").tobytes()
    alpha_mask = _encode_png(Image.frombytes("L", source.size, source_alpha))
    width, height = source.size
    anchors = {
        layer_id: (width // 2, height // 2) for layer_id in SEMANTIC_LAYER_IDS
    }
    masks = {layer_id: alpha_mask for layer_id in SEMANTIC_LAYER_IDS}
    return ModuleSemanticSnapshot(
        body_module_id=body_module_id,
        module_contract_sha256=hashlib.sha256(module_contract).hexdigest(),
        width=width,
        height=height,
        source_alpha=source_alpha,
        source_alpha_sha256=hashlib.sha256(source_alpha).hexdigest(),
        layer_masks=masks,
        layer_mask_sha256={
            layer_id: hashlib.sha256(mask).hexdigest()
            for layer_id, mask in masks.items()
        },
        layer_anchors=anchors,
        seam_dilation_px=2,
    )


def _decode_neutral(neutral_png: bytes) -> Image.Image:
    try:
        with Image.open(BytesIO(neutral_png)) as source:
            source.load()
            if source.format != "PNG":
                raise ValueError("neutral texture must be a PNG")
            if source.size != _PNG_SIZE:
                raise ValueError("neutral texture must be exactly 2048x2048")
            if "A" not in source.getbands():
                raise ValueError("neutral texture must have an alpha channel")
            return source.copy()
    except (OSError, SyntaxError) as exc:
        raise ValueError("neutral texture must be a valid PNG") from exc


def _encode_png(image: Image.Image) -> bytes:
    output = BytesIO()
    image.save(output, format="PNG", optimize=False, compress_level=9)
    return output.getvalue()


def generate_guides(repo_root: Path) -> dict[str, Any]:
    module_root = (
        repo_root
        / "apps"
        / "desktop"
        / "public"
        / "cat-character-modules"
        / "cat-a-live2d-v1"
    )
    output_root = (
        repo_root
        / "services"
        / "appearance-generation"
        / "src"
        / "photo_avatar_backend"
        / "assets"
        / "uv-guides"
    )
    output_root.mkdir(parents=True, exist_ok=True)
    index_path = output_root / "索引.json"
    reviews = _existing_reviews(index_path)
    guides: list[dict[str, Any]] = []

    for module_id in BODY_MODULE_IDS:
        module_dir = module_root / module_id
        contract_path = module_dir / "模块.json"
        contract_bytes = contract_path.read_bytes()
        contract = json.loads(contract_bytes.decode("utf-8"))
        neutral_relative = contract.get("files", {}).get("neutralTexture")
        if contract.get("moduleId") != module_id or not isinstance(neutral_relative, str):
            raise ValueError(f"invalid module contract: {module_id}")
        neutral_path = resolve_module_file(module_dir, neutral_relative)
        neutral_bytes = neutral_path.read_bytes()
        work_bundle = build_work_canvas(neutral_bytes)
        legacy_guide_path = output_root / f"{module_id}.png"
        work_canvas_path = output_root / f"{module_id}.work.png"
        region_map_path = output_root / f"{module_id}.regions.png"
        legacy_guide_path.unlink(missing_ok=True)
        work_canvas_path.write_bytes(work_bundle.work_canvas_png)
        region_map_path.write_bytes(work_bundle.region_map_png)
        work_canvas_sha256 = hashlib.sha256(work_bundle.work_canvas_png).hexdigest()
        region_map_sha256 = hashlib.sha256(work_bundle.region_map_png).hexdigest()
        source_texture_sha256 = hashlib.sha256(neutral_bytes).hexdigest()
        alpha_sha256 = work_bundle.source_alpha_sha256
        previous = reviews.get(module_id)
        visual_review = {
            "status": "pending",
            "conclusion": "尚未完成人工视觉检查",
        }
        if (
            isinstance(previous, Mapping)
            and previous.get("sourceAlphaSha256") == alpha_sha256
            and previous.get("workCanvasSha256") == work_canvas_sha256
            and previous.get("regionMapSha256") == region_map_sha256
            and previous.get("sourceTextureSha256") == source_texture_sha256
            and isinstance(previous.get("visualReview"), Mapping)
        ):
            visual_review = previous["visualReview"]
        guides.append(
            {
                "moduleId": module_id,
                "workCanvasPath": work_canvas_path.name,
                "regionMapPath": region_map_path.name,
                "workCanvasSha256": work_canvas_sha256,
                "regionMapSha256": region_map_sha256,
                "sourceTextureSha256": source_texture_sha256,
                "moduleContractSha256": hashlib.sha256(contract_bytes).hexdigest(),
                "sourceAlphaSha256": alpha_sha256,
                "width": _PNG_SIZE[0],
                "height": _PNG_SIZE[1],
                "visualReview": visual_review,
            }
        )

    index = {
        "schemaVersion": 2,
        "generatorVersion": GENERATOR_VERSION,
        "guides": guides,
    }
    index_path.write_text(
        json.dumps(index, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return index


def _existing_reviews(index_path: Path) -> dict[str, Any]:
    if not index_path.is_file():
        return {}
    try:
        index = json.loads(index_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return {}
    guides = index.get("guides") if isinstance(index, Mapping) else None
    if not isinstance(guides, list):
        return {}
    return {
        entry["moduleId"]: entry
        for entry in guides
        if isinstance(entry, Mapping)
        and isinstance(entry.get("moduleId"), str)
        and isinstance(entry.get("visualReview"), Mapping)
    }


def _main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, required=True)
    args = parser.parse_args()
    index = generate_guides(args.repo_root.resolve())
    for guide in index["guides"]:
        print(
            f"{guide['moduleId']} {guide['workCanvasSha256']} "
            f"{guide['width']}x{guide['height']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())

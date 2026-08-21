from __future__ import annotations

from dataclasses import dataclass
import hashlib
from io import BytesIO
from typing import Mapping, Sequence

from PIL import Image

from .contracts import ContractError


SEMANTIC_LAYER_IDS = (
    "body-base",
    "face",
    "eyes-eyelids",
    "ears",
    "chest-forelegs",
    "tail",
    "occlusion-underlay",
)
_SEMANTIC_LAYER_ID_SET = frozenset(SEMANTIC_LAYER_IDS)


@dataclass(frozen=True)
class SemanticLayerSpec:
    layer_id: str
    width: int
    height: int
    anchor_x: int
    anchor_y: int
    mask_png: bytes
    mask_sha256: str
    seam_dilation_px: int


@dataclass(frozen=True)
class ModuleSemanticSnapshot:
    body_module_id: str
    module_contract_sha256: str
    width: int
    height: int
    source_alpha: bytes
    source_alpha_sha256: str
    layer_masks: Mapping[str, bytes]
    layer_mask_sha256: Mapping[str, str]
    layer_anchors: Mapping[str, tuple[int, int]]
    seam_dilation_px: int


@dataclass(frozen=True)
class ValidatedSemanticLayer:
    layer_id: str
    canonical_png: bytes
    rgba: bytes
    provider_raw_sha256: str
    canonical_layer_sha256: str
    mask_sha256: str
    anchor_x: int
    anchor_y: int


def decode_png_exact(png: bytes, *, mode: str, size: tuple[int, int]) -> Image.Image:
    try:
        with Image.open(BytesIO(png)) as image:
            if image.format != "PNG":
                raise ContractError("semantic layer input must be a PNG")
            image.load()
            if image.mode != mode:
                raise ContractError(f"semantic layer input must use {mode} mode")
            if image.size != size:
                raise ContractError("semantic layer dimensions do not match the module")
            return image.copy()
    except ContractError:
        raise
    except (OSError, SyntaxError, ValueError) as exc:
        raise ContractError("semantic layer input must be a valid PNG") from exc


def build_semantic_layer_specs(
    module_snapshot: ModuleSemanticSnapshot,
) -> Sequence[SemanticLayerSpec]:
    expected = set(SEMANTIC_LAYER_IDS)
    if set(module_snapshot.layer_masks) != expected:
        raise ContractError("semantic layer masks do not match the fixed layer set")
    if set(module_snapshot.layer_mask_sha256) != expected:
        raise ContractError("semantic layer mask hashes do not match the fixed layer set")
    if set(module_snapshot.layer_anchors) != expected:
        raise ContractError("semantic layer anchors do not match the fixed layer set")
    if hashlib.sha256(module_snapshot.source_alpha).hexdigest() != module_snapshot.source_alpha_sha256:
        raise ContractError("module source alpha hash does not match")
    if len(module_snapshot.source_alpha) != module_snapshot.width * module_snapshot.height:
        raise ContractError("module source alpha dimensions do not match")
    if not 0 <= module_snapshot.seam_dilation_px <= 32:
        raise ContractError("semantic seam dilation is outside the allowed range")

    specs: list[SemanticLayerSpec] = []
    for layer_id in SEMANTIC_LAYER_IDS:
        anchor_x, anchor_y = module_snapshot.layer_anchors[layer_id]
        mask_png = module_snapshot.layer_masks[layer_id]
        mask_sha256 = hashlib.sha256(mask_png).hexdigest()
        if mask_sha256 != module_snapshot.layer_mask_sha256[layer_id]:
            raise ContractError(f"semantic layer mask hash does not match: {layer_id}")
        if not 0 <= anchor_x < module_snapshot.width or not 0 <= anchor_y < module_snapshot.height:
            raise ContractError(f"semantic layer anchor is outside the atlas: {layer_id}")
        mask = decode_png_exact(
            mask_png,
            mode="L",
            size=(module_snapshot.width, module_snapshot.height),
        )
        if any(
            mask_value and not module_snapshot.source_alpha[index]
            for index, mask_value in enumerate(mask.tobytes())
        ):
            raise ContractError(f"semantic layer mask escapes module alpha: {layer_id}")
        specs.append(
            SemanticLayerSpec(
                layer_id=layer_id,
                width=module_snapshot.width,
                height=module_snapshot.height,
                anchor_x=anchor_x,
                anchor_y=anchor_y,
                mask_png=mask_png,
                mask_sha256=mask_sha256,
                seam_dilation_px=module_snapshot.seam_dilation_px,
            )
        )
    return tuple(specs)


def validate_semantic_layer_png(
    png: bytes,
    spec: SemanticLayerSpec,
) -> ValidatedSemanticLayer:
    if spec.layer_id not in _SEMANTIC_LAYER_ID_SET:
        raise ContractError(f"unknown semantic layer ID: {spec.layer_id}")
    layer = decode_png_exact(png, mode="RGBA", size=(spec.width, spec.height))
    mask = decode_png_exact(spec.mask_png, mode="L", size=(spec.width, spec.height))
    if hashlib.sha256(spec.mask_png).hexdigest() != spec.mask_sha256:
        raise ContractError(f"semantic layer mask hash does not match: {spec.layer_id}")
    rgba = layer.tobytes()
    alpha = layer.getchannel("A").tobytes()
    for index, mask_value in enumerate(mask.tobytes()):
        if mask_value == 0 and alpha[index] != 0:
            raise ContractError(f"semantic layer escapes its mask: {spec.layer_id}")
    output = BytesIO()
    layer.save(output, format="PNG", optimize=False, compress_level=9)
    canonical_png = output.getvalue()
    return ValidatedSemanticLayer(
        layer_id=spec.layer_id,
        canonical_png=canonical_png,
        rgba=rgba,
        provider_raw_sha256=hashlib.sha256(png).hexdigest(),
        canonical_layer_sha256=hashlib.sha256(canonical_png).hexdigest(),
        mask_sha256=spec.mask_sha256,
        anchor_x=spec.anchor_x,
        anchor_y=spec.anchor_y,
    )

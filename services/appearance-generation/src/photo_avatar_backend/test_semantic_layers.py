from __future__ import annotations

import hashlib
from io import BytesIO

import pytest
from PIL import Image

from .contracts import ContractError
from .semantic_layers import (
    SEMANTIC_LAYER_IDS,
    ModuleSemanticSnapshot,
    SemanticLayerSpec,
    build_semantic_layer_specs,
    validate_semantic_layer_png,
)


def _png(mode: str, pixels: bytes) -> bytes:
    image = Image.frombytes(mode, (2, 2), pixels)
    output = BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


def _snapshot() -> ModuleSemanticSnapshot:
    source_alpha = b"\xff\xff\xff\xff"
    mask_png = _png("L", source_alpha)
    masks = {layer_id: mask_png for layer_id in SEMANTIC_LAYER_IDS}
    return ModuleSemanticSnapshot(
        body_module_id="body-balanced-v1",
        module_contract_sha256="a" * 64,
        width=2,
        height=2,
        source_alpha=source_alpha,
        source_alpha_sha256=hashlib.sha256(source_alpha).hexdigest(),
        layer_masks=masks,
        layer_mask_sha256={
            layer_id: hashlib.sha256(mask).hexdigest()
            for layer_id, mask in masks.items()
        },
        layer_anchors={layer_id: (1, 1) for layer_id in SEMANTIC_LAYER_IDS},
        seam_dilation_px=2,
    )


def test_build_specs_contains_each_fixed_semantic_layer_once():
    specs = build_semantic_layer_specs(_snapshot())

    assert tuple(spec.layer_id for spec in specs) == SEMANTIC_LAYER_IDS
    assert len({spec.layer_id for spec in specs}) == len(SEMANTIC_LAYER_IDS)
    assert all((spec.width, spec.height, spec.anchor_x, spec.anchor_y) == (2, 2, 1, 1) for spec in specs)


def test_build_specs_rejects_unknown_layer_set():
    snapshot = _snapshot()
    masks = dict(snapshot.layer_masks)
    masks.pop("tail")
    masks["unknown"] = next(iter(snapshot.layer_masks.values()))

    with pytest.raises(ContractError, match="fixed layer set"):
        build_semantic_layer_specs(
            ModuleSemanticSnapshot(
                **{**snapshot.__dict__, "layer_masks": masks}
            )
        )


def test_build_specs_rejects_mask_hash_not_bound_to_snapshot():
    snapshot = _snapshot()
    hashes = dict(snapshot.layer_mask_sha256)
    hashes["face"] = "0" * 64

    with pytest.raises(ContractError, match="mask hash"):
        build_semantic_layer_specs(
            ModuleSemanticSnapshot(
                **{**snapshot.__dict__, "layer_mask_sha256": hashes}
            )
        )


def test_validate_layer_rejects_unknown_layer_id():
    spec = SemanticLayerSpec(
        layer_id="unknown",
        width=2,
        height=2,
        anchor_x=1,
        anchor_y=1,
        mask_png=_png("L", b"\xff\xff\xff\xff"),
        mask_sha256=hashlib.sha256(_png("L", b"\xff\xff\xff\xff")).hexdigest(),
        seam_dilation_px=2,
    )

    with pytest.raises(ContractError, match="unknown semantic layer ID"):
        validate_semantic_layer_png(_png("RGBA", b"\x00" * 16), spec)


def test_validate_layer_rejects_pixels_outside_mask():
    mask_png = _png("L", b"\xff\x00\x00\x00")
    spec = SemanticLayerSpec(
        layer_id="face",
        width=2,
        height=2,
        anchor_x=0,
        anchor_y=0,
        mask_png=mask_png,
        mask_sha256=hashlib.sha256(mask_png).hexdigest(),
        seam_dilation_px=2,
    )
    layer_png = _png(
        "RGBA",
        b"\x00\x00\x00\x00\x01\x02\x03\xff\x00\x00\x00\x00\x00\x00\x00\x00",
    )

    with pytest.raises(ContractError, match="escapes its mask"):
        validate_semantic_layer_png(layer_png, spec)

from __future__ import annotations

import hashlib
from collections.abc import Callable
from io import BytesIO

import pytest
from PIL import Image

from .contracts import ContractError
from .semantic_layers import (
    ModuleSemanticSnapshot,
    SEMANTIC_LAYER_IDS,
    ValidatedSemanticLayer,
    build_semantic_layer_specs,
    validate_semantic_layer_png,
)
from .texture_compositor import (
    EXPECTED_LAYER_ORDER,
    compose_canonical_texture,
    compose_semantic_atlas,
)


def png(
    mode: str,
    size: tuple[int, int],
    pixel: Callable[[int, int], int | tuple[int, ...]],
) -> bytes:
    image = Image.new(mode, size)
    image.putdata([pixel(x, y) for y in range(size[1]) for x in range(size[0])])
    output = BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


def _compose(
    *,
    size: tuple[int, int],
    regions: bytes,
    alpha: bytes,
    provider_pixel=lambda x, y: (20 + x, 40 + y, 80),
    canvas_pixel=lambda x, y: (255, 0, 255, 255),
    minimum_change_ratio: float = 0.95,
):
    width, height = size
    return compose_canonical_texture(
        provider_png=png("RGB", size, provider_pixel),
        work_canvas_png=png("RGBA", size, canvas_pixel),
        region_map_png=png("L", size, lambda x, y: regions[y * width + x]),
        module_alpha=alpha,
        minimum_change_ratio=minimum_change_ratio,
    )


def _alpha(size: tuple[int, int], value_at: Callable[[int, int], int]) -> bytes:
    width, height = size
    return bytes(value_at(x, y) for y in range(height) for x in range(width))


def _regions(size: tuple[int, int], value_at: Callable[[int, int], int]) -> bytes:
    width, height = size
    return bytes(value_at(x, y) for y in range(height) for x in range(width))


@pytest.mark.parametrize(
    ("size", "regions", "alpha"),
    [
        (
            (6, 6),
            _regions((6, 6), lambda x, y: 1 if 1 <= x <= 4 and 1 <= y <= 4 else 0),
            _alpha((6, 6), lambda x, y: 255 if 1 <= x <= 4 and 1 <= y <= 4 else 0),
        ),
        (
            (6, 6),
            _regions(
                (6, 6),
                lambda x, y: 1
                if 1 <= x <= 2 and 1 <= y <= 2
                else 2
                if 4 <= x <= 5 and 3 <= y <= 4
                else 0,
            ),
            _alpha(
                (6, 6),
                lambda x, y: 255
                if (1 <= x <= 2 and 1 <= y <= 2)
                or (4 <= x <= 5 and 3 <= y <= 4)
                else 0,
            ),
        ),
        (
            (7, 7),
            _regions(
                (7, 7),
                lambda x, y: 3
                if 1 <= x <= 5 and 1 <= y <= 5 and not (2 <= x <= 4 and 2 <= y <= 4)
                else 0,
            ),
            _alpha(
                (7, 7),
                lambda x, y: 255
                if 1 <= x <= 5 and 1 <= y <= 5 and not (2 <= x <= 4 and 2 <= y <= 4)
                else 0,
            ),
        ),
        (
            (6, 6),
            _regions((6, 6), lambda x, y: 4 if 1 <= x <= 4 and 1 <= y <= 4 else 0),
            _alpha(
                (6, 6),
                lambda x, y: 128
                if (x in (1, 4) or y in (1, 4)) and 1 <= x <= 4 and 1 <= y <= 4
                else 255
                if 2 <= x <= 3 and 2 <= y <= 3
                else 0,
            ),
        ),
        (
            (5, 5),
            _regions((5, 5), lambda x, y: 5 if x <= 2 and y <= 3 else 0),
            _alpha((5, 5), lambda x, y: 255 if x <= 2 and y <= 3 else 0),
        ),
        (
            (5, 5),
            _regions((5, 5), lambda x, y: 6 if 1 <= x <= 3 and 1 <= y <= 3 else 0),
            _alpha(
                (5, 5),
                lambda x, y: 64 + x * 32 if 1 <= x <= 3 and 1 <= y <= 3 else 0,
            ),
        ),
    ],
    ids=[
        "continuous-contour",
        "disconnected-islands",
        "hole",
        "semi-transparent-edge",
        "edge-touching-contour",
        "different-alpha-control",
    ],
)
def test_composer_passes_topology_fixture_pixel_for_pixel(
    size: tuple[int, int], regions: bytes, alpha: bytes
):
    result = _compose(size=size, regions=regions, alpha=alpha)

    image = Image.open(BytesIO(result.png)).convert("RGBA")
    assert image.size == size
    assert image.getchannel("A").tobytes() == alpha
    for y in range(size[1]):
        for x in range(size[0]):
            expected_alpha = alpha[y * size[0] + x]
            if expected_alpha:
                assert image.getpixel((x, y)) == (20 + x, 40 + y, 80, expected_alpha)
            else:
                assert image.getpixel((x, y)) == (0, 0, 0, 0)
    assert result.coverage.to_wire()["regions"]


def test_composer_uses_only_module_alpha_and_zeros_hidden_rgb():
    provider = png("RGB", (8, 8), lambda x, y: (20 + x, 40 + y, 80))
    canvas = png("RGBA", (8, 8), lambda x, y: (255, 0, 255, 255))
    regions = png("L", (8, 8), lambda x, y: 1 if 1 <= x <= 7 else 0)
    alpha = bytes(0 if x == 0 else 128 if x == 1 else 255 for y in range(8) for x in range(8))

    result = compose_canonical_texture(
        provider_png=provider,
        work_canvas_png=canvas,
        region_map_png=regions,
        module_alpha=alpha,
        minimum_change_ratio=0.95,
    )

    image = Image.open(BytesIO(result.png)).convert("RGBA")
    assert image.getpixel((0, 0)) == (0, 0, 0, 0)
    assert image.getpixel((1, 0))[3] == 128
    assert image.getchannel("A").tobytes() == alpha


def test_composer_rejects_provider_rgba_when_any_alpha_is_not_255():
    provider = png("RGBA", (2, 2), lambda x, y: (20, 40, 80, 254 if x == y == 0 else 255))
    canvas = png("RGBA", (2, 2), lambda x, y: (255, 0, 255, 255))
    regions = png("L", (2, 2), lambda x, y: 1)
    alpha = bytes([255, 255, 255, 255])

    with pytest.raises(ContractError, match="opaque"):
        compose_canonical_texture(
            provider_png=provider,
            work_canvas_png=canvas,
            region_map_png=regions,
            module_alpha=alpha,
        )


def test_composer_rejects_region_change_ratio_below_minimum():
    size = (10, 1)
    regions = bytes([1] * 10)
    alpha = bytes([255] * 10)

    with pytest.raises(ContractError, match="change ratio"):
        _compose(
            size=size,
            regions=regions,
            alpha=alpha,
            provider_pixel=lambda x, y: (10, 20, 30),
            canvas_pixel=lambda x, y: (10, 20, 30, 255) if x == 0 else (0, 0, 0, 255),
            minimum_change_ratio=0.95,
        )


@pytest.mark.parametrize(
    "minimum_change_ratio",
    [
        pytest.param(float("nan"), id="nan"),
        pytest.param(0.949, id="below-hard-minimum"),
        pytest.param(0, id="zero"),
    ],
)
def test_composer_rejects_minimum_change_ratio_outside_hard_contract(
    minimum_change_ratio: float,
):
    size = (2, 2)
    regions = bytes([1] * 4)
    alpha = bytes([255] * 4)

    with pytest.raises(ContractError, match="minimum change ratio"):
        _compose(
            size=size,
            regions=regions,
            alpha=alpha,
            minimum_change_ratio=minimum_change_ratio,
        )


def test_composer_rejects_all_black_region_map():
    size = (4, 4)
    regions = bytes([0] * 16)
    alpha = bytes([0] * 16)

    with pytest.raises(ContractError, match="region"):
        _compose(size=size, regions=regions, alpha=alpha)


def test_composer_rejects_region_map_and_alpha_mismatch():
    size = (3, 3)
    regions = _regions(size, lambda x, y: 1 if x == 1 and y == 1 else 0)
    alpha = bytes([0] * 9)

    with pytest.raises(ContractError, match="alpha"):
        _compose(size=size, regions=regions, alpha=alpha)


def test_composer_rejects_visible_alpha_without_region_id():
    size = (3, 3)
    regions = bytes([0] * 9)
    alpha = _alpha(size, lambda x, y: 255 if x == 1 and y == 1 else 0)

    with pytest.raises(ContractError, match="region"):
        _compose(size=size, regions=regions, alpha=alpha)


def test_composer_rejects_region_pixels_that_are_all_black():
    size = (3, 3)
    regions = _regions(size, lambda x, y: 1 if 1 <= x <= 2 and 1 <= y <= 2 else 0)
    alpha = _alpha(size, lambda x, y: 255 if 1 <= x <= 2 and 1 <= y <= 2 else 0)

    with pytest.raises(ContractError, match="black"):
        _compose(
            size=size,
            regions=regions,
            alpha=alpha,
            provider_pixel=lambda x, y: (0, 0, 0),
        )


def test_composer_is_deterministic_for_identical_inputs():
    size = (4, 4)
    regions = _regions(size, lambda x, y: 1 if 1 <= x <= 2 and 1 <= y <= 2 else 0)
    alpha = _alpha(size, lambda x, y: 255 if 1 <= x <= 2 and 1 <= y <= 2 else 0)
    provider = png("RGB", size, lambda x, y: (30 + x, 60 + y, 90))
    canvas = png("RGBA", size, lambda x, y: (255, 0, 255, 255))
    region_map = png("L", size, lambda x, y: regions[y * size[0] + x])

    first = compose_canonical_texture(
        provider_png=provider,
        work_canvas_png=canvas,
        region_map_png=region_map,
        module_alpha=alpha,
    )
    second = compose_canonical_texture(
        provider_png=provider,
        work_canvas_png=canvas,
        region_map_png=region_map,
        module_alpha=alpha,
    )

    assert first.png == second.png
    assert first.canonical_sha256 == second.canonical_sha256
    assert first.provider_raw_sha256 == hashlib.sha256(provider).hexdigest()
    assert first.source_alpha_sha256 == hashlib.sha256(alpha).hexdigest()
    assert first.canonical_sha256 == hashlib.sha256(first.png).hexdigest()
    assert first.coverage.to_wire() == second.coverage.to_wire()


def _semantic_snapshot(
    module_id: str,
    *,
    size: tuple[int, int],
    alpha: bytes,
    masks: dict[str, bytes] | None = None,
    dilation: int = 1,
) -> ModuleSemanticSnapshot:
    width, height = size
    default_mask = png("L", size, lambda x, y: alpha[y * width + x])
    layer_masks = masks or {layer_id: default_mask for layer_id in SEMANTIC_LAYER_IDS}
    return ModuleSemanticSnapshot(
        body_module_id=module_id,
        module_contract_sha256="11" * 32,
        width=width,
        height=height,
        source_alpha=alpha,
        source_alpha_sha256=hashlib.sha256(alpha).hexdigest(),
        layer_masks=layer_masks,
        layer_mask_sha256={
            layer_id: hashlib.sha256(mask).hexdigest()
            for layer_id, mask in layer_masks.items()
        },
        layer_anchors={layer_id: (0, 0) for layer_id in SEMANTIC_LAYER_IDS},
        seam_dilation_px=dilation,
    )


def _semantic_layers(
    snapshot: ModuleSemanticSnapshot,
    colors: dict[str, tuple[int, int, int]],
    visible: Callable[[str, int, int], bool],
) -> tuple[ValidatedSemanticLayer, ...]:
    layers = []
    for spec in build_semantic_layer_specs(snapshot):
        layer_id = spec.layer_id
        mask = Image.open(BytesIO(snapshot.layer_masks[layer_id])).convert("L")
        layer_png = png(
            "RGBA",
            (snapshot.width, snapshot.height),
            lambda x, y: (*colors.get(layer_id, (0, 0, 0)), mask.getpixel((x, y)))
            if visible(layer_id, x, y)
            else (0, 0, 0, 0),
        )
        layers.append(validate_semantic_layer_png(layer_png, spec))
    return tuple(layers)


@pytest.mark.parametrize(
    "module_id",
    ("body-slender-v1", "body-balanced-v1", "body-rounded-v1"),
)
@pytest.mark.parametrize(
    "alpha",
    (
        _alpha((6, 6), lambda x, y: 255 if 1 <= x <= 4 and 1 <= y <= 4 else 0),
        _alpha(
            (6, 6),
            lambda x, y: 255
            if (1 <= x <= 2 and 1 <= y <= 2) or (4 <= x <= 5 and 3 <= y <= 4)
            else 0,
        ),
        _alpha(
            (6, 6),
            lambda x, y: 255
            if 1 <= x <= 4 and 1 <= y <= 4 and not (2 <= x <= 3 and 2 <= y <= 3)
            else 0,
        ),
        _alpha(
            (6, 6),
            lambda x, y: 128
            if (x in (1, 4) or y in (1, 4)) and 1 <= x <= 4 and 1 <= y <= 4
            else 255
            if 2 <= x <= 3 and 2 <= y <= 3
            else 0,
        ),
        _alpha((6, 6), lambda x, y: 255 if x <= 2 and y <= 3 else 0),
    ),
    ids=("continuous", "islands", "hole", "semi-transparent", "edge-touching"),
)
def test_semantic_atlas_preserves_topology_for_each_body_module(
    module_id: str, alpha: bytes
):
    snapshot = _semantic_snapshot(module_id, size=(6, 6), alpha=alpha)
    layers = _semantic_layers(
        snapshot,
        {"body-base": (31, 63, 95)},
        lambda layer_id, _x, _y: layer_id == "body-base",
    )

    first = compose_semantic_atlas(layers=layers, module_snapshot=snapshot)
    second = compose_semantic_atlas(layers=layers, module_snapshot=snapshot)

    with Image.open(BytesIO(first.png)) as atlas:
        rgba = atlas.convert("RGBA")
        assert rgba.getchannel("A").tobytes() == alpha
        assert all(pixel[:3] == (0, 0, 0) for pixel in rgba.getdata() if pixel[3] == 0)
    assert first.png == second.png
    assert first.canonical_sha256 == hashlib.sha256(first.png).hexdigest()
    assert first.source_alpha_sha256 == snapshot.source_alpha_sha256
    assert first.transparent_rgb_is_zero is True
    assert first.layer_order == EXPECTED_LAYER_ORDER


@pytest.mark.parametrize(
    ("lower", "upper"),
    (("body-base", "chest-forelegs"), ("face", "eyes-eyelids")),
)
def test_semantic_atlas_uses_fixed_overlap_order(lower: str, upper: str):
    alpha = bytes([255] * 9)
    snapshot = _semantic_snapshot("body-balanced-v1", size=(3, 3), alpha=alpha)
    colors = {lower: (10, 20, 30), upper: (200, 210, 220)}
    layers = _semantic_layers(
        snapshot,
        colors,
        lambda layer_id, x, y: layer_id in colors and x == y == 1,
    )

    result = compose_semantic_atlas(layers=layers, module_snapshot=snapshot)

    with Image.open(BytesIO(result.png)) as atlas:
        assert atlas.convert("RGBA").getpixel((1, 1)) == (200, 210, 220, 255)


def test_semantic_atlas_expands_rgb_only_inside_the_authoritative_mask():
    alpha = bytes([255] * 25)
    snapshot = _semantic_snapshot("body-rounded-v1", size=(5, 5), alpha=alpha)
    layers = _semantic_layers(
        snapshot,
        {"body-base": (70, 80, 90)},
        lambda layer_id, x, y: layer_id == "body-base" and x == y == 2,
    )

    result = compose_semantic_atlas(layers=layers, module_snapshot=snapshot)

    with Image.open(BytesIO(result.png)) as atlas:
        rgba = atlas.convert("RGBA")
        assert rgba.getpixel((1, 2)) == (70, 80, 90, 255)
        assert rgba.getpixel((0, 0)) == (0, 0, 0, 255)

from __future__ import annotations

import hashlib
import math
from collections.abc import Sequence
from dataclasses import dataclass
from io import BytesIO

from PIL import Image, __version__ as PILLOW_VERSION

from .contracts import ContractError
from .semantic_layers import ModuleSemanticSnapshot, ValidatedSemanticLayer, decode_png_exact


COMPOSER_VERSION = "deterministic-alpha-v1"
PNG_ENCODER_VERSION = f"pillow-{PILLOW_VERSION}"
EXPECTED_LAYER_ORDER = (
    "body-base",
    "occlusion-underlay",
    "chest-forelegs",
    "tail",
    "face",
    "ears",
    "eyes-eyelids",
)


@dataclass(frozen=True)
class RegionCoverage:
    region_id: int
    pixel_count: int
    changed_pixels: int
    change_ratio: float


@dataclass(frozen=True)
class CoverageReport:
    minimum_change_ratio: float
    regions: tuple[RegionCoverage, ...]

    def to_wire(self) -> dict[str, object]:
        return {
            "minimumChangeRatio": self.minimum_change_ratio,
            "regions": [
                {
                    "regionId": item.region_id,
                    "pixelCount": item.pixel_count,
                    "changedPixels": item.changed_pixels,
                    "changeRatio": item.change_ratio,
                }
                for item in self.regions
            ],
        }


@dataclass(frozen=True)
class CanonicalTexture:
    png: bytes
    provider_raw_sha256: str
    canonical_sha256: str
    source_alpha_sha256: str
    coverage: CoverageReport


@dataclass(frozen=True)
class SemanticAtlas:
    png: bytes
    canonical_sha256: str
    source_alpha_sha256: str
    transparent_rgb_is_zero: bool
    layer_order: Sequence[str]


def compose_semantic_atlas(
    *,
    layers: Sequence[ValidatedSemanticLayer],
    module_snapshot: ModuleSemanticSnapshot,
) -> SemanticAtlas:
    expected = set(EXPECTED_LAYER_ORDER)
    layer_by_id = {layer.layer_id: layer for layer in layers}
    if len(layers) != len(expected) or set(layer_by_id) != expected:
        raise ContractError("semantic atlas requires each fixed layer exactly once")
    pixel_count = module_snapshot.width * module_snapshot.height
    if len(module_snapshot.source_alpha) != pixel_count:
        raise ContractError("module source alpha dimensions do not match")
    source_alpha_sha256 = hashlib.sha256(module_snapshot.source_alpha).hexdigest()
    if source_alpha_sha256 != module_snapshot.source_alpha_sha256:
        raise ContractError("module source alpha hash does not match")
    if set(module_snapshot.layer_masks) != expected:
        raise ContractError("semantic layer masks do not match the fixed layer set")
    if set(module_snapshot.layer_mask_sha256) != expected:
        raise ContractError("semantic layer mask hashes do not match the fixed layer set")
    if not 0 <= module_snapshot.seam_dilation_px <= 32:
        raise ContractError("semantic seam dilation is outside the allowed range")

    composed_rgb = bytearray(pixel_count * 3)
    for layer_id in EXPECTED_LAYER_ORDER:
        layer = layer_by_id[layer_id]
        mask_png = module_snapshot.layer_masks[layer_id]
        mask_sha256 = hashlib.sha256(mask_png).hexdigest()
        if (
            mask_sha256 != module_snapshot.layer_mask_sha256[layer_id]
            or layer.mask_sha256 != mask_sha256
        ):
            raise ContractError(f"semantic layer mask hash does not match: {layer_id}")
        mask = decode_png_exact(
            mask_png,
            mode="L",
            size=(module_snapshot.width, module_snapshot.height),
        ).tobytes()
        decoded = decode_png_exact(
            layer.canonical_png,
            mode="RGBA",
            size=(module_snapshot.width, module_snapshot.height),
        )
        if hashlib.sha256(layer.canonical_png).hexdigest() != layer.canonical_layer_sha256:
            raise ContractError(f"semantic layer canonical hash does not match: {layer_id}")
        rgba = decoded.tobytes()
        if rgba != layer.rgba or len(rgba) != pixel_count * 4:
            raise ContractError(f"semantic layer pixels do not match: {layer_id}")
        expanded_rgb, coverage = _expand_layer_rgb(
            rgba,
            mask,
            module_snapshot.source_alpha,
            module_snapshot.width,
            module_snapshot.height,
            module_snapshot.seam_dilation_px,
            layer_id,
        )
        for index, covered in enumerate(coverage):
            if covered:
                source_offset = index * 3
                composed_rgb[source_offset : source_offset + 3] = expanded_rgb[
                    source_offset : source_offset + 3
                ]

    rgba = bytearray(pixel_count * 4)
    for index, alpha in enumerate(module_snapshot.source_alpha):
        rgb_offset = index * 3
        rgba_offset = index * 4
        if alpha:
            rgba[rgba_offset : rgba_offset + 3] = composed_rgb[rgb_offset : rgb_offset + 3]
        rgba[rgba_offset + 3] = alpha
    image = Image.frombytes(
        "RGBA", (module_snapshot.width, module_snapshot.height), bytes(rgba)
    )
    output = BytesIO()
    image.save(output, format="PNG", optimize=False, compress_level=9)
    png_bytes = output.getvalue()
    return SemanticAtlas(
        png=png_bytes,
        canonical_sha256=hashlib.sha256(png_bytes).hexdigest(),
        source_alpha_sha256=source_alpha_sha256,
        transparent_rgb_is_zero=all(
            alpha or rgba[index * 4 : index * 4 + 3] == b"\x00\x00\x00"
            for index, alpha in enumerate(module_snapshot.source_alpha)
        ),
        layer_order=EXPECTED_LAYER_ORDER,
    )


def _expand_layer_rgb(
    rgba: bytes,
    mask: bytes,
    source_alpha: bytes,
    width: int,
    height: int,
    radius: int,
    layer_id: str,
) -> tuple[bytes, bytes]:
    source_coverage = bytearray(width * height)
    rgb = bytearray(width * height * 3)
    for index, mask_value in enumerate(mask):
        alpha = rgba[index * 4 + 3]
        if mask_value and not source_alpha[index]:
            raise ContractError(f"semantic mask escapes module alpha: {layer_id}")
        if alpha and not mask_value:
            raise ContractError(f"semantic layer escapes its mask: {layer_id}")
        if alpha:
            source_coverage[index] = 1
            rgb[index * 3 : index * 3 + 3] = rgba[index * 4 : index * 4 + 3]
    coverage = bytearray(source_coverage)
    offsets = sorted(
        (
            (dx * dx + dy * dy, dy, dx)
            for dy in range(-radius, radius + 1)
            for dx in range(-radius, radius + 1)
            if dx or dy
        )
    )
    for index, mask_value in enumerate(mask):
        if not mask_value or coverage[index]:
            continue
        x = index % width
        y = index // width
        for _, dy, dx in offsets:
            source_x = x + dx
            source_y = y + dy
            if not 0 <= source_x < width or not 0 <= source_y < height:
                continue
            source_index = source_y * width + source_x
            if source_coverage[source_index]:
                rgb[index * 3 : index * 3 + 3] = rgb[
                    source_index * 3 : source_index * 3 + 3
                ]
                coverage[index] = 1
                break
    return bytes(rgb), bytes(coverage)


def compose_canonical_texture(
    *,
    provider_png: bytes,
    work_canvas_png: bytes,
    region_map_png: bytes,
    module_alpha: bytes,
    minimum_change_ratio: float = 0.95,
) -> CanonicalTexture:
    provider = _decode_provider_rgb(provider_png)
    canvas = _decode_rgba(work_canvas_png, "work canvas")
    regions = _decode_region_map(region_map_png)
    if provider.size != canvas.size or provider.size != regions.size:
        raise ContractError("texture composition dimensions do not match")
    if len(module_alpha) != provider.width * provider.height:
        raise ContractError("module alpha length does not match texture")
    coverage = _coverage(provider, canvas, regions, module_alpha, minimum_change_ratio)
    provider_rgb = provider.tobytes()
    rgba = bytearray(provider.width * provider.height * 4)
    for index, alpha in enumerate(module_alpha):
        source_offset = index * 3
        offset = index * 4
        if alpha:
            rgba[offset : offset + 3] = provider_rgb[source_offset : source_offset + 3]
        rgba[offset + 3] = alpha
    image = Image.frombytes("RGBA", provider.size, bytes(rgba))
    output = BytesIO()
    image.save(output, format="PNG", optimize=False, compress_level=9)
    png_bytes = output.getvalue()
    return CanonicalTexture(
        png=png_bytes,
        provider_raw_sha256=hashlib.sha256(provider_png).hexdigest(),
        canonical_sha256=hashlib.sha256(png_bytes).hexdigest(),
        source_alpha_sha256=hashlib.sha256(module_alpha).hexdigest(),
        coverage=coverage,
    )


def _decode_provider_rgb(png: bytes) -> Image.Image:
    image = _open_png(png, "provider")
    if image.mode == "RGB":
        return image.copy()
    if image.mode == "RGBA":
        if any(alpha != 255 for alpha in image.getchannel("A").tobytes()):
            raise ContractError("provider RGBA must be fully opaque")
        return image.convert("RGB")
    raise ContractError("provider texture must be RGB or fully opaque RGBA")


def _decode_rgba(png: bytes, label: str) -> Image.Image:
    image = _open_png(png, label)
    if image.mode != "RGBA":
        raise ContractError(f"{label} must be RGBA")
    return image.copy()


def _decode_region_map(png: bytes) -> Image.Image:
    image = _open_png(png, "region map")
    if image.mode != "L":
        raise ContractError("region map must be L")
    return image.copy()


def _open_png(png: bytes, label: str) -> Image.Image:
    try:
        with Image.open(BytesIO(png)) as image:
            if image.format != "PNG":
                raise ContractError(f"{label} must be a PNG")
            image.load()
            return image.copy()
    except ContractError:
        raise
    except (OSError, SyntaxError, ValueError) as exc:
        raise ContractError(f"{label} must be a valid PNG") from exc


def _coverage(
    provider: Image.Image,
    canvas: Image.Image,
    regions: Image.Image,
    module_alpha: bytes,
    minimum_change_ratio: float,
) -> CoverageReport:
    if not math.isfinite(minimum_change_ratio) or not 0.95 <= minimum_change_ratio <= 1.0:
        raise ContractError("minimum change ratio is outside the allowed range")

    provider_rgb = provider.tobytes()
    canvas_rgba = canvas.tobytes()
    region_ids = regions.tobytes()
    stats: dict[int, list[int | bool]] = {}

    for index, region_id in enumerate(region_ids):
        alpha = module_alpha[index]
        if alpha > 0 and region_id == 0:
            raise ContractError("visible alpha pixel is missing a region id")
        if region_id == 0:
            continue
        if alpha == 0:
            raise ContractError("region map does not match module alpha")
        region = stats.setdefault(region_id, [0, 0, False])
        region[0] = int(region[0]) + 1
        provider_offset = index * 3
        canvas_offset = index * 4
        provider_pixel = provider_rgb[provider_offset : provider_offset + 3]
        canvas_pixel = canvas_rgba[canvas_offset : canvas_offset + 3]
        if provider_pixel != canvas_pixel:
            region[1] = int(region[1]) + 1
        if provider_pixel != b"\x00\x00\x00":
            region[2] = True

    if not stats:
        raise ContractError("region map must contain at least one region")

    coverage: list[RegionCoverage] = []
    for region_id in sorted(stats):
        pixel_count = int(stats[region_id][0])
        changed_pixels = int(stats[region_id][1])
        if not bool(stats[region_id][2]):
            raise ContractError("region pixels are all black")
        change_ratio = changed_pixels / pixel_count
        if change_ratio < minimum_change_ratio:
            raise ContractError("region change ratio is below the minimum")
        coverage.append(
            RegionCoverage(
                region_id=region_id,
                pixel_count=pixel_count,
                changed_pixels=changed_pixels,
                change_ratio=change_ratio,
            )
        )
    return CoverageReport(
        minimum_change_ratio=minimum_change_ratio,
        regions=tuple(coverage),
    )

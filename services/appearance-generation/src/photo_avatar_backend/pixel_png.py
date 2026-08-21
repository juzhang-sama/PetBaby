from dataclasses import dataclass
import hashlib
from io import BytesIO
import math
from typing import assert_never

from PIL import Image

from .contracts import ContractError
from .pixel_audit import PixelAlphaReportV1
from .pixel_mask import largest_component, remove_edge_checkerboard
from .pixel_palette import PixelPaletteReportV2, quantize_animation_ready_rgba
from .pixel_style import PixelPostprocessProfile


_MAX_ARTIFACT_BYTES = 20 * 1024 * 1024
_PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


@dataclass(frozen=True, slots=True)
class PixelPngAudit:
    normalized_png: bytes
    provider_raw_sha256: str
    normalized_sha256: str
    width: int
    height: int
    alpha_report: PixelAlphaReportV1

    @property
    def largest_component_share(self) -> float:
        return self.alpha_report.largest_component_share

    @property
    def partial_alpha_ratio(self) -> float:
        return self.alpha_report.partial_alpha_ratio


@dataclass(frozen=True, slots=True)
class PixelPostprocessResult:
    png: bytes
    palette_report: PixelPaletteReportV2 | None


def audit_pixel_png(png: bytes) -> PixelPngAudit:
    if len(png) > _MAX_ARTIFACT_BYTES:
        raise ContractError("pixel artifact exceeds 20 MiB")
    if not png.startswith(_PNG_SIGNATURE):
        raise ContractError("pixel artifact must be PNG")
    try:
        with Image.open(BytesIO(png)) as image:
            if image.format != "PNG" or image.mode not in {"RGB", "RGBA"}:
                raise ContractError("pixel artifact must be RGB or RGBA PNG")
            width, height = image.size
            if not 1024 <= width <= 2048 or not 1024 <= height <= 2048:
                raise ContractError("pixel artifact dimensions are outside 1024..2048")
            if width * height > 4_194_304:
                raise ContractError("pixel artifact dimensions exceed the pixel limit")
            image.load()
            rgba = image.convert("RGBA")
            if image.mode == "RGB":
                remove_edge_checkerboard(rgba)
    except ContractError:
        raise
    except (OSError, SyntaxError, ValueError) as exc:
        raise ContractError("pixel artifact is not a valid PNG") from exc

    alpha = rgba.getchannel("A")
    alpha_bytes = alpha.tobytes()
    bounds = alpha.getbbox()
    if bounds is None:
        raise ContractError("pixel artifact has no visible pixels")
    left, top, right, bottom = bounds
    margins = (left, top, width - right, height - bottom)
    minimum_x_margin = math.ceil(width * 0.02)
    minimum_y_margin = math.ceil(height * 0.02)
    if (
        margins[0] < minimum_x_margin
        or margins[2] < minimum_x_margin
        or margins[1] < minimum_y_margin
        or margins[3] < minimum_y_margin
    ):
        raise ContractError("pixel artifact alpha margin is below 2 percent")

    visible_pixels = sum(value > 0 for value in alpha_bytes)
    partial_alpha_pixels = sum(0 < value < 255 for value in alpha_bytes)
    partial_alpha_ratio = partial_alpha_pixels / visible_pixels
    if partial_alpha_ratio > 0.02:
        raise ContractError("pixel artifact partial alpha ratio exceeds 2 percent")
    largest_component_pixels = largest_component(alpha_bytes, width, height)
    largest_component_share = largest_component_pixels / visible_pixels
    if largest_component_share < 0.95:
        raise ContractError("pixel artifact connected subject share is below 95 percent")

    normalized_bytes = bytearray(rgba.tobytes())
    for pixel_index, alpha_value in enumerate(alpha_bytes):
        if alpha_value == 0:
            offset = pixel_index * 4
            normalized_bytes[offset] = 0
            normalized_bytes[offset + 1] = 0
            normalized_bytes[offset + 2] = 0
    normalized = Image.frombytes("RGBA", (width, height), bytes(normalized_bytes))
    output = BytesIO()
    normalized.save(output, format="PNG", compress_level=9, optimize=False)
    normalized_png = output.getvalue()
    report = PixelAlphaReportV1(
        visible_pixels=visible_pixels,
        partial_alpha_pixels=partial_alpha_pixels,
        partial_alpha_ratio=partial_alpha_ratio,
        largest_component_pixels=largest_component_pixels,
        largest_component_share=largest_component_share,
        bounds_left=left,
        bounds_top=top,
        bounds_right=right,
        bounds_bottom=bottom,
        margin_left=margins[0],
        margin_top=margins[1],
        margin_right=margins[2],
        margin_bottom=margins[3],
    )
    PixelAlphaReportV1.from_wire(report.to_wire())
    return PixelPngAudit(
        normalized_png=normalized_png,
        provider_raw_sha256=hashlib.sha256(png).hexdigest(),
        normalized_sha256=hashlib.sha256(normalized_png).hexdigest(),
        width=width,
        height=height,
        alpha_report=report,
    )


def normalize_pixel_png(png: bytes, safe_margin_ratio: float = 0.04) -> bytes:
    if not 0.02 <= safe_margin_ratio < 0.5:
        raise ContractError("pixel safe margin ratio is outside 0.02..0.5")
    if len(png) > _MAX_ARTIFACT_BYTES or not png.startswith(_PNG_SIGNATURE):
        raise ContractError("pixel artifact must be PNG")
    try:
        with Image.open(BytesIO(png)) as image:
            if image.format != "PNG" or image.mode not in {"RGB", "RGBA"}:
                raise ContractError("pixel artifact must be RGB or RGBA PNG")
            width, height = image.size
            if not 1024 <= width <= 2048 or not 1024 <= height <= 2048:
                raise ContractError("pixel artifact dimensions are outside 1024..2048")
            rgba = image.convert("RGBA")
            if image.mode == "RGB":
                remove_edge_checkerboard(rgba)
            bounds = rgba.getchannel("A").getbbox()
            if bounds is None:
                raise ContractError("pixel artifact has no visible pixels")
            subject = rgba.crop(bounds)
    except ContractError:
        raise
    except (OSError, SyntaxError, ValueError) as exc:
        raise ContractError("pixel artifact is not a valid PNG") from exc

    margin_x = math.ceil(width * safe_margin_ratio)
    margin_y = math.ceil(height * safe_margin_ratio)
    available_width = width - 2 * margin_x
    available_height = height - 2 * margin_y
    scale = min(available_width / subject.width, available_height / subject.height, 1.0)
    resized_size = (
        max(1, math.floor(subject.width * scale)),
        max(1, math.floor(subject.height * scale)),
    )
    if resized_size != subject.size:
        subject = subject.resize(resized_size, Image.Resampling.NEAREST)

    red, green, blue, alpha = subject.split()
    hard_alpha = alpha.point(lambda value: 255 if value >= 128 else 0)
    subject = Image.merge("RGBA", (red, green, blue, hard_alpha))
    subject_bytes = bytearray(subject.tobytes())
    for offset in range(0, len(subject_bytes), 4):
        if subject_bytes[offset + 3] == 0:
            subject_bytes[offset : offset + 3] = b"\x00\x00\x00"
    subject = Image.frombytes("RGBA", subject.size, bytes(subject_bytes))

    normalized = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    normalized.alpha_composite(
        subject,
        ((width - subject.width) // 2, (height - subject.height) // 2),
    )
    output = BytesIO()
    normalized.save(output, format="PNG", compress_level=9, optimize=False)
    return output.getvalue()


_PIXELATE_TARGET_SIZE = 512
_PIXELATE_PALETTE_COLORS = 32


def pixelate_pixel_png(
    png: bytes,
    target_size: int = _PIXELATE_TARGET_SIZE,
    palette_colors: int = _PIXELATE_PALETTE_COLORS,
) -> bytes:
    """把标准化后的 RGBA PNG 转成真像素风（确定性后处理）。

    gpt-image-2 无法靠 prompt 稳定锁定像素风（软绘 + 噪点伪装），这里用确定性的
    后处理把任意输入图转成「大像素簇 + 有限色阶 + 硬边」：

    1. RGB 降采样到 target_size（BOX 面积平均，每个像素块取区域平均色）
    2. RGB 量化到 palette_colors 色调色板（有限色阶）
    3. NEAREST 放大回原尺寸（硬边像素簇）
    4. alpha 通道用多数投票（BOX 平均 + 阈值）保持硬边透明背景

    alpha 与透明背景完全不变（不影响 downstream 的 alpha 审计、连通组件、
    边缘留白），只重画颜色。像素化后主体尺寸/位置不变。
    """
    if not 2 <= target_size <= 1024:
        raise ContractError("pixelate target size is outside 2..1024")
    if not 2 <= palette_colors <= 256:
        raise ContractError("pixelate palette colors is outside 2..256")
    try:
        with Image.open(BytesIO(png)) as image:
            rgba = image.convert("RGBA")
            width, height = rgba.size
    except (OSError, SyntaxError, ValueError) as exc:
        raise ContractError("pixel artifact is not a valid PNG") from exc

    # 1) RGB 降采样 + 量化到有限调色板
    small_rgb = rgba.convert("RGB").resize(
        (target_size, target_size), Image.Resampling.BOX
    )
    small_rgb = small_rgb.quantize(
        colors=palette_colors, method=Image.Quantize.MEDIANCUT
    ).convert("RGB")

    # 2) alpha 多数投票降采样：BOX 平均后 >= 128 视为不透明（不透明占比 >= 50%）
    small_alpha = rgba.getchannel("A").resize(
        (target_size, target_size), Image.Resampling.BOX
    )
    small_alpha = small_alpha.point(lambda value: 255 if value >= 128 else 0)

    # 3) NEAREST 放大回原尺寸
    big_rgb = small_rgb.resize((width, height), Image.Resampling.NEAREST)
    big_alpha = small_alpha.resize((width, height), Image.Resampling.NEAREST)

    # 4) 合并 + 透明像素 RGB 清零
    merged = Image.merge("RGBA", (*big_rgb.split(), big_alpha))
    merged_bytes = bytearray(merged.tobytes())
    for offset in range(0, len(merged_bytes), 4):
        if merged_bytes[offset + 3] == 0:
            merged_bytes[offset : offset + 3] = b"\x00\x00\x00"
    merged = Image.frombytes("RGBA", merged.size, bytes(merged_bytes))

    output = BytesIO()
    merged.save(output, format="PNG", compress_level=9, optimize=False)
    return output.getvalue()


def postprocess_pixel_png(
    png: bytes,
    postprocess: PixelPostprocessProfile,
) -> PixelPostprocessResult:
    if postprocess.downsample != "box" or postprocess.upsample != "nearest":
        raise ContractError("pixel postprocess resampling profile is invalid")
    if postprocess.dither != "none":
        raise ContractError("pixel postprocess dither profile is invalid")
    match postprocess.quantize_method:
        case "mediancut":
            return PixelPostprocessResult(
                png=pixelate_pixel_png(
                    png,
                    target_size=postprocess.logical_grid_size,
                    palette_colors=postprocess.palette_color_limit,
                ),
                palette_report=None,
            )
        case "maxcoverage":
            pass
        case unreachable:
            assert_never(unreachable)

    try:
        with Image.open(BytesIO(png)) as image:
            rgba = image.convert("RGBA")
    except (OSError, SyntaxError, ValueError) as exc:
        raise ContractError("pixel artifact is not a valid PNG") from exc
    logical_size = (postprocess.logical_grid_size, postprocess.logical_grid_size)
    logical = rgba.resize(logical_size, Image.Resampling.BOX)
    hard_alpha = rgba.getchannel("A").resize(
        logical_size, Image.Resampling.BOX
    ).point(lambda value: 255 if value >= 128 else 0)
    logical.putalpha(hard_alpha)
    quantized, report = quantize_animation_ready_rgba(logical, postprocess)
    exported = quantized.resize((1024, 1024), Image.Resampling.NEAREST)
    output = BytesIO()
    exported.save(output, format="PNG", compress_level=9, optimize=False)
    return PixelPostprocessResult(png=output.getvalue(), palette_report=report)



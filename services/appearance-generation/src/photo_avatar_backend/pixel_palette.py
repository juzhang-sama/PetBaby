import math
from collections.abc import Sequence
from dataclasses import dataclass

from PIL import Image

from .contracts import ContractError
from .pixel_style import PixelPostprocessProfile


Rgb = tuple[int, int, int]
Lab = tuple[float, float, float]
VisiblePixel = tuple[int, Rgb]


@dataclass(frozen=True, slots=True)
class PixelPaletteReportV2:
    visible_color_count: int
    protected_accent_count: int


@dataclass(frozen=True, slots=True)
class _ClusterQuantization:
    labels: tuple[int, ...]
    colors: tuple[Rgb, ...]
    shares: tuple[float, ...]


def delta_e_ciede2000(first: Lab, second: Lab) -> float:
    lightness_1, green_red_1, blue_yellow_1 = first
    lightness_2, green_red_2, blue_yellow_2 = second
    chroma_1 = math.hypot(green_red_1, blue_yellow_1)
    chroma_2 = math.hypot(green_red_2, blue_yellow_2)
    mean_chroma = (chroma_1 + chroma_2) / 2.0
    mean_chroma_7 = mean_chroma**7
    adjustment = 0.5 * (
        1.0 - math.sqrt(mean_chroma_7 / (mean_chroma_7 + 25.0**7))
    )
    adjusted_red_1 = (1.0 + adjustment) * green_red_1
    adjusted_red_2 = (1.0 + adjustment) * green_red_2
    adjusted_chroma_1 = math.hypot(adjusted_red_1, blue_yellow_1)
    adjusted_chroma_2 = math.hypot(adjusted_red_2, blue_yellow_2)
    hue_1 = math.degrees(math.atan2(blue_yellow_1, adjusted_red_1)) % 360.0
    hue_2 = math.degrees(math.atan2(blue_yellow_2, adjusted_red_2)) % 360.0

    delta_lightness = lightness_2 - lightness_1
    delta_chroma = adjusted_chroma_2 - adjusted_chroma_1
    hue_difference = hue_2 - hue_1
    if adjusted_chroma_1 * adjusted_chroma_2 == 0.0:
        delta_hue = 0.0
    elif abs(hue_difference) <= 180.0:
        delta_hue = hue_difference
    elif hue_difference > 180.0:
        delta_hue = hue_difference - 360.0
    else:
        delta_hue = hue_difference + 360.0
    delta_hue_term = 2.0 * math.sqrt(
        adjusted_chroma_1 * adjusted_chroma_2
    ) * math.sin(math.radians(delta_hue / 2.0))

    mean_lightness = (lightness_1 + lightness_2) / 2.0
    mean_adjusted_chroma = (adjusted_chroma_1 + adjusted_chroma_2) / 2.0
    if adjusted_chroma_1 * adjusted_chroma_2 == 0.0:
        mean_hue = hue_1 + hue_2
    elif abs(hue_difference) <= 180.0:
        mean_hue = (hue_1 + hue_2) / 2.0
    elif hue_1 + hue_2 < 360.0:
        mean_hue = (hue_1 + hue_2 + 360.0) / 2.0
    else:
        mean_hue = (hue_1 + hue_2 - 360.0) / 2.0

    hue_weight = (
        1.0
        - 0.17 * math.cos(math.radians(mean_hue - 30.0))
        + 0.24 * math.cos(math.radians(2.0 * mean_hue))
        + 0.32 * math.cos(math.radians(3.0 * mean_hue + 6.0))
        - 0.20 * math.cos(math.radians(4.0 * mean_hue - 63.0))
    )
    lightness_scale = 1.0 + (
        0.015 * (mean_lightness - 50.0) ** 2
    ) / math.sqrt(20.0 + (mean_lightness - 50.0) ** 2)
    chroma_scale = 1.0 + 0.045 * mean_adjusted_chroma
    hue_scale = 1.0 + 0.015 * mean_adjusted_chroma * hue_weight
    rotation_angle = 30.0 * math.exp(-((mean_hue - 275.0) / 25.0) ** 2)
    chroma_rotation = 2.0 * math.sqrt(
        mean_adjusted_chroma**7 / (mean_adjusted_chroma**7 + 25.0**7)
    )
    rotation = -math.sin(math.radians(2.0 * rotation_angle)) * chroma_rotation
    lightness_term = delta_lightness / lightness_scale
    chroma_term = delta_chroma / chroma_scale
    hue_term = delta_hue_term / hue_scale
    return math.sqrt(
        lightness_term**2
        + chroma_term**2
        + hue_term**2
        + rotation * chroma_term * hue_term
    )


def delta_e_rgb(first: Rgb, second: Rgb) -> float:
    return delta_e_ciede2000(_rgb_to_lab(first), _rgb_to_lab(second))


def quantize_animation_ready_rgba(
    image: Image.Image,
    profile: PixelPostprocessProfile,
) -> tuple[Image.Image, PixelPaletteReportV2]:
    rgba = image.convert("RGBA")
    pixels = tuple(rgba.get_flattened_data())
    visible = tuple(
        (index, (red, green, blue))
        for index, (red, green, blue, alpha) in enumerate(pixels)
        if alpha > 0
    )
    if not visible:
        raise ContractError("pixel artifact has no visible pixels")
    if not 1 <= profile.palette_color_limit <= 256:
        raise ContractError("pixel palette color limit is invalid")
    if not 0 <= profile.protected_accent_slots < profile.palette_color_limit:
        raise ContractError("pixel protected accent slots are invalid")

    clusters = _quantize_visible(visible, colors=64)
    main = tuple(
        index for index, share in enumerate(clusters.shares) if share >= 0.05
    )
    candidates = [
        index
        for index, share in enumerate(clusters.shares)
        if 0.0005 <= share < 0.05
        and main
        and min(
            delta_e_rgb(clusters.colors[index], clusters.colors[main_index])
            for main_index in main
        )
        >= 18.0
    ]
    candidates.sort(
        key=lambda index: (
            -min(
                delta_e_rgb(clusters.colors[index], clusters.colors[main_index])
                for main_index in main
            )
            * math.sqrt(clusters.shares[index]),
            clusters.colors[index],
        )
    )
    protected = tuple(candidates[: profile.protected_accent_slots])
    protected_set = frozenset(protected)
    remaining = tuple(
        pixel
        for pixel, label in zip(visible, clusters.labels, strict=True)
        if label not in protected_set
    )
    output: list[tuple[int, int, int, int]] = [(0, 0, 0, 0)] * len(pixels)

    if remaining:
        base = _quantize_visible(
            remaining,
            colors=profile.palette_color_limit - len(protected),
        )
        for (pixel_index, _), label in zip(remaining, base.labels, strict=True):
            red, green, blue = base.colors[label]
            output[pixel_index] = (red, green, blue, pixels[pixel_index][3])

    for (pixel_index, _), label in zip(visible, clusters.labels, strict=True):
        if label in protected_set:
            red, green, blue = clusters.colors[label]
            output[pixel_index] = (red, green, blue, pixels[pixel_index][3])

    result = Image.new("RGBA", rgba.size, (0, 0, 0, 0))
    result.putdata(output)
    visible_colors = {
        (red, green, blue)
        for red, green, blue, alpha in output
        if alpha > 0
    }
    if len(visible_colors) > profile.palette_color_limit:
        raise ContractError("pixel artifact exceeds the visible color limit")
    for protected_index in protected:
        protected_color = clusters.colors[protected_index]
        if min(delta_e_rgb(protected_color, color) for color in visible_colors) > 12.0:
            raise ContractError("pixel protected accent color was not preserved")
    return result, PixelPaletteReportV2(
        visible_color_count=len(visible_colors),
        protected_accent_count=len(protected),
    )


def _quantize_visible(
    visible: Sequence[VisiblePixel],
    colors: int,
) -> _ClusterQuantization:
    rgb_values = tuple(rgb for _, rgb in visible)
    strip = Image.new("RGB", (len(rgb_values), 1))
    strip.putdata(rgb_values)
    quantized = strip.quantize(
        colors=min(colors, len(rgb_values)),
        method=Image.Quantize.MAXCOVERAGE,
        dither=Image.Dither.NONE,
    )
    palette = quantized.getpalette()
    if palette is None:
        raise ContractError("pixel quantizer did not return a palette")
    palette_labels = tuple(int(label) for label in quantized.get_flattened_data())
    used_labels = tuple(sorted(set(palette_labels)))
    compact_index = {label: index for index, label in enumerate(used_labels)}
    compact_labels = tuple(compact_index[label] for label in palette_labels)
    compact_colors = tuple(
        (palette[label * 3], palette[label * 3 + 1], palette[label * 3 + 2])
        for label in used_labels
    )
    counts = [0] * len(compact_colors)
    for label in compact_labels:
        counts[label] += 1
    total = len(compact_labels)
    return _ClusterQuantization(
        labels=compact_labels,
        colors=compact_colors,
        shares=tuple(count / total for count in counts),
    )


def _rgb_to_lab(rgb: Rgb) -> Lab:
    linear = tuple(
        channel / 12.92
        if channel <= 0.04045
        else ((channel + 0.055) / 1.055) ** 2.4
        for channel in (value / 255.0 for value in rgb)
    )
    red, green, blue = linear
    x = (0.4124564 * red + 0.3575761 * green + 0.1804375 * blue) / 0.95047
    y = 0.2126729 * red + 0.7151522 * green + 0.0721750 * blue
    z = (0.0193339 * red + 0.1191920 * green + 0.9503041 * blue) / 1.08883

    def lab_curve(value: float) -> float:
        if value > 216.0 / 24389.0:
            return value ** (1.0 / 3.0)
        return (841.0 / 108.0) * value + 4.0 / 29.0

    x_curve = lab_curve(x)
    y_curve = lab_curve(y)
    z_curve = lab_curve(z)
    return (
        116.0 * y_curve - 16.0,
        500.0 * (x_curve - y_curve),
        200.0 * (y_curve - z_curve),
    )

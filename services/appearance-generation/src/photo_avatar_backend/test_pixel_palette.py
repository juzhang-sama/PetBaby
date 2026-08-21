from io import BytesIO

from PIL import Image, ImageDraw
import pytest

from .pixel_palette import (
    delta_e_ciede2000,
    quantize_animation_ready_rgba,
)
from .pixel_png import postprocess_pixel_png
from .pixel_style import load_pixel_style_pack


V2_POSTPROCESS = load_pixel_style_pack("pixel-style-v2-animation-ready").postprocess


def _visible_rgb_colors(image: Image.Image) -> set[tuple[int, int, int]]:
    rgba = image.convert("RGBA")
    return {
        (red, green, blue)
        for red, green, blue, alpha in rgba.get_flattened_data()
        if alpha > 0
    }


def _sparse_rgba() -> Image.Image:
    image = Image.new("RGBA", (160, 160), (0, 0, 0, 0))
    ImageDraw.Draw(image).rectangle((40, 30, 119, 149), fill=(60, 70, 80, 255))
    return image


def _source_png() -> bytes:
    image = Image.new("RGBA", (1024, 1024), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rectangle((96, 64, 927, 959), fill=(72, 62, 54, 255))
    draw.rectangle((250, 180, 773, 850), fill=(220, 214, 198, 255))
    draw.rectangle((398, 388, 423, 413), fill=(92, 152, 80, 255))
    draw.rectangle((600, 388, 625, 413), fill=(92, 152, 80, 255))
    output = BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


def test_ciede2000_matches_sharma_reference_vector() -> None:
    # Given
    first = (50.0, 2.6772, -79.7751)
    second = (50.0, 0.0, -82.7485)

    # When
    difference = delta_e_ciede2000(first, second)

    # Then
    assert difference == pytest.approx(2.0425, abs=1e-4)


def test_v2_palette_preserves_small_green_eye_accents() -> None:
    # Given
    image = Image.new("RGBA", (160, 160), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rectangle((20, 20, 139, 149), fill=(72, 62, 54, 255))
    draw.rectangle((28, 28, 131, 140), fill=(220, 214, 198, 255))
    draw.rectangle((63, 62, 66, 65), fill=(92, 152, 80, 255))
    draw.rectangle((93, 62, 96, 65), fill=(92, 152, 80, 255))

    # When
    result, report = quantize_animation_ready_rgba(image, V2_POSTPROCESS)

    # Then
    assert report.visible_color_count <= 24
    assert 1 <= report.protected_accent_count <= 4
    assert (92, 152, 80) in _visible_rgb_colors(result)


def test_transparent_black_does_not_consume_palette_slot() -> None:
    # Given
    image = _sparse_rgba()

    # When
    result, report = quantize_animation_ready_rgba(image, V2_POSTPROCESS)

    # Then
    colors = _visible_rgb_colors(result)
    assert report.visible_color_count == len(colors)
    assert (0, 0, 0) not in colors


def test_v2_postprocess_is_byte_deterministic() -> None:
    # Given
    source = _source_png()

    # When
    first = postprocess_pixel_png(source, V2_POSTPROCESS)
    second = postprocess_pixel_png(source, V2_POSTPROCESS)

    # Then
    assert first.png == second.png
    assert first.palette_report == second.palette_report

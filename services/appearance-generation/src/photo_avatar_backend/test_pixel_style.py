import pytest

from .pixel_style import (
    DEFAULT_PIXEL_STYLE_ID,
    PixelStyleError,
    load_pixel_style_pack,
)


def test_v2_is_default_while_v1_remains_explicitly_loadable() -> None:
    current = load_pixel_style_pack("pixel-style-v2-animation-ready")
    historical = load_pixel_style_pack("pixel-style-v1")

    assert DEFAULT_PIXEL_STYLE_ID == "pixel-style-v2-animation-ready"
    assert current.style_profile_id == "pixel-style-v2-animation-ready"
    assert current.postprocess.logical_grid_size == 160
    assert current.postprocess.palette_color_limit == 24
    assert current.postprocess.quantize_method == "maxcoverage"
    assert historical.style_profile_id == "pixel-style-v1"
    assert historical.postprocess.logical_grid_size == 512
    assert historical.postprocess.palette_color_limit == 32


def test_style_loader_rejects_unknown_id() -> None:
    with pytest.raises(PixelStyleError, match="unsupported pixel style"):
        load_pixel_style_pack("pixel-style-v3")


def test_new_generation_default_is_v2_after_cutover() -> None:
    assert DEFAULT_PIXEL_STYLE_ID == "pixel-style-v2-animation-ready"
    assert load_pixel_style_pack().style_profile_id == DEFAULT_PIXEL_STYLE_ID

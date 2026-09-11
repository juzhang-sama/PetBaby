import pytest

from .pixel_style import (
    DEFAULT_PIXEL_STYLE_ID,
    KNOWN_PIXEL_STYLE_IDS,
    RETIRED_PIXEL_STYLE_IDS,
    SUPPORTED_PIXEL_STYLE_IDS,
    PixelStyleError,
    load_pixel_style_pack,
)


def test_v2_is_the_only_generation_style() -> None:
    current = load_pixel_style_pack("pixel-style-v2-animation-ready")

    assert DEFAULT_PIXEL_STYLE_ID == "pixel-style-v2-animation-ready"
    assert SUPPORTED_PIXEL_STYLE_IDS == {"pixel-style-v2-animation-ready"}
    assert RETIRED_PIXEL_STYLE_IDS == {"pixel-style-v1"}
    # 「已知」比「可用」多一个 v1：历史档案/审计里 v1 是合法取值，读得出来，但生成用不了。
    assert KNOWN_PIXEL_STYLE_IDS == {
        "pixel-style-v1",
        "pixel-style-v2-animation-ready",
    }
    assert SUPPORTED_PIXEL_STYLE_IDS.isdisjoint(RETIRED_PIXEL_STYLE_IDS)
    assert current.style_profile_id == "pixel-style-v2-animation-ready"
    assert current.postprocess.logical_grid_size == 160
    assert current.postprocess.palette_color_limit == 24
    assert current.postprocess.quantize_method == "maxcoverage"


@pytest.mark.parametrize("style_id", sorted(RETIRED_PIXEL_STYLE_IDS))
def test_retired_style_cannot_be_loaded_for_generation(style_id: str) -> None:
    # 回归锁：已淘汰风格必须被风格加载器硬拒，否则它会借任何直调路径复活
    # （2026-09-11 之前 load_pixel_style_pack("pixel-style-v1") 是静默可用的）。
    with pytest.raises(PixelStyleError, match="retired"):
        load_pixel_style_pack(style_id)


def test_style_loader_rejects_unknown_id() -> None:
    with pytest.raises(PixelStyleError, match="unsupported pixel style"):
        load_pixel_style_pack("pixel-style-v3")


def test_new_generation_default_is_v2_after_cutover() -> None:
    assert DEFAULT_PIXEL_STYLE_ID == "pixel-style-v2-animation-ready"
    assert load_pixel_style_pack().style_profile_id == DEFAULT_PIXEL_STYLE_ID

"""Tests for the standard cat body template contract."""

import copy
import json
from pathlib import Path

import pytest

from .standard_cat_template import (
    AMPLITUDE_SEMANTICS,
    StandardCatTemplateError,
    load_standard_cat_template,
)


def _template_root() -> Path:
    return Path(__file__).parent / "assets" / "pixel-style-v1"


def _load_raw() -> dict:
    path = _template_root() / "标准猫体模板.json"
    return json.loads(path.read_text(encoding="utf-8"))


def test_loads_checked_in_template_with_stable_identity() -> None:
    template = load_standard_cat_template()

    assert template.template_id == "standard-cat-seated-v1"
    assert template.engine_profile == "life-v1"
    assert template.data["schemaVersion"] == 1
    assert template.data["canvas"] == {"width": 256, "height": 256}
    assert template.template_sha256  # 供审计/跨层校验使用的指纹
    assert len(template.template_sha256) == 64


def test_style_selection_loads_the_independent_v2_template() -> None:
    current = load_standard_cat_template("pixel-style-v2-animation-ready")
    historical = load_standard_cat_template("pixel-style-v1")

    assert current.template_id == "standard-cat-seated-v1"
    assert historical.template_id == "standard-cat-seated-v1"
    assert current.template_sha256 == historical.template_sha256


def test_proportions_match_1_8_head_template() -> None:
    template = load_standard_cat_template()
    proportions = template.data["proportions"]
    head = proportions["headHeightFraction"]
    body = proportions["bodyHeightFraction"]
    # 1.8 头身：头占身高约 55%，身体约 45%
    assert 0.5 <= head <= 0.6
    assert abs(head + body - 1.0) <= 0.05


def test_parts_are_layered_and_unique() -> None:
    template = load_standard_cat_template()
    parts = template.data["parts"]
    ids = [part["id"] for part in parts]
    layers = [part["layer"] for part in parts]
    assert len(ids) == len(set(ids))
    assert len(layers) == len(set(layers))
    assert "head" in ids and "body" in ids and "tail" in ids
    # 头在最上层，尾巴在最底层
    by_id = {part["id"]: part for part in parts}
    assert by_id["head"]["layer"] > by_id["body"]["layer"] > by_id["tail"]["layer"]


def test_space_anchors_are_normalized_and_consistent() -> None:
    template = load_standard_cat_template()
    space = template.data["space"]
    for side in ("left", "right"):
        eye = space["eyes"][side]
        assert 0.0 <= eye["center"]["x"] <= 1.0
        assert 0.0 <= eye["center"]["y"] <= 1.0
        assert eye["bounds"]["left"] < eye["bounds"]["right"]
        assert eye["bounds"]["top"] < eye["bounds"]["bottom"]
    alpha = space["alphaBounds"]
    head = next(part for part in template.data["parts"] if part["id"] == "head")
    # 头部必须整体落在 alpha 包围盒内
    assert head["bounds"]["left"] >= alpha["left"]
    assert head["bounds"]["top"] >= alpha["top"]
    assert head["bounds"]["right"] <= alpha["right"]
    assert head["bounds"]["bottom"] <= alpha["bottom"]


def test_amplitude_covers_all_semantics_with_valid_ranges() -> None:
    template = load_standard_cat_template()
    amplitude = template.data["amplitude"]
    assert set(amplitude.keys()) == set(AMPLITUDE_SEMANTICS)
    for semantic in AMPLITUDE_SEMANTICS:
        low, high = template.amplitude(semantic)
        assert low <= high


@pytest.mark.parametrize(
    "mutate,message",
    [
        (lambda d: d.__setitem__("schemaVersion", 2), "schemaVersion"),
        (lambda d: d["canvas"].__setitem__("width", -1), "canvas"),
        (lambda d: d["proportions"].__setitem__("headHeightFraction", 1.2), "proportions"),
        (lambda d: d["parts"].clear(), "parts"),
        (lambda d: d["parts"][0].__setitem__("layer", 3), "layer"),
        (lambda d: d["space"]["eyes"]["left"]["center"].__setitem__("x", 1.5), "eyes"),
        (lambda d: d["amplitude"].__setitem__("breath", {"min": 0.5, "max": 0.1}), "amplitude"),
    ],
)
def test_rejects_invalid_templates(mutate, message) -> None:
    raw = _load_raw()
    mutate(raw)
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "pixel-style-v1"
        root.mkdir()
        (root / "标准猫体模板.json").write_text(
            json.dumps(raw, ensure_ascii=False), encoding="utf-8"
        )
        with pytest.raises(StandardCatTemplateError, match=message):
            load_standard_cat_template(root)


def test_missing_template_file_is_rejected() -> None:
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        with pytest.raises(StandardCatTemplateError, match="missing"):
            load_standard_cat_template(Path(tmp) / "pixel-style-v1")


def test_load_returns_a_fresh_copy_each_time() -> None:
    first = load_standard_cat_template()
    second = load_standard_cat_template()
    assert first.data is not second.data
    first.data["proportions"]["headHeightFraction"] = 0.1
    assert second.data["proportions"]["headHeightFraction"] != 0.1

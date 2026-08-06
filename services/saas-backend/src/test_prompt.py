# -*- coding: utf-8 -*-
import pytest

from src.prompt import STYLE_3D, STYLE_CARTOON, build_guided_prompt, build_prompt


def test_build_prompt_mentions_pure_white_background() -> None:
    prompt = build_prompt("cat")
    assert "pure white background" in prompt
    assert "no shadow" in prompt
    assert "no watermark" in prompt


def test_build_prompt_includes_species() -> None:
    assert "a dog" in build_prompt("dog")


def test_build_prompt_keeps_identity_fidelity() -> None:
    prompt = build_prompt("cat")
    assert "High fidelity to the reference" in prompt
    assert "face proportions" in prompt


def test_build_guided_prompt_includes_chosen_traits() -> None:
    prompt = build_guided_prompt(
        "cat",
        {
            "body": "round",
            "fur": "short",
            "color": "orange",
            "pattern": "striped",
            "face": "round face with big eyes",
            "accessory": "red bow",
        },
    )
    assert "a cat" in prompt
    assert "orange" in prompt
    assert "striped" in prompt
    assert "red bow" in prompt
    assert "pure white background" in prompt


def test_build_guided_prompt_skips_none_accessory() -> None:
    prompt = build_guided_prompt(
        "dog",
        {
            "body": "round",
            "fur": "long",
            "color": "cream",
            "pattern": "solid",
            "face": "sleepy gentle eyes",
            "accessory": "none",
        },
    )
    assert "none" not in prompt


def test_3d_style_prompt_uses_3d_block() -> None:
    prompt = build_prompt("cat", style=STYLE_3D)
    assert "3D rendered pet" in prompt
    assert "clay-like fur" in prompt
    assert "pure white background" in prompt
    assert "High fidelity to the reference" in prompt


def test_3d_guided_prompt_uses_3d_block() -> None:
    prompt = build_guided_prompt("cat", {"color": "orange"}, style=STYLE_3D)
    assert "3D rendered pet" in prompt
    assert "orange" in prompt


def test_unknown_style_rejected() -> None:
    with pytest.raises(ValueError):
        build_prompt("cat", style="unknown")
    with pytest.raises(ValueError):
        build_guided_prompt("cat", {}, style="unknown")


def test_cartoon_style_is_default() -> None:
    assert build_prompt("cat", style=STYLE_CARTOON) == build_prompt("cat")

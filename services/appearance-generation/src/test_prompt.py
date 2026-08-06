# -*- coding: utf-8 -*-
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from prompt import (  # noqa: E402
    STYLE_CONSTRAINTS,
    STYLE_3D_CONSTRAINTS,
    STYLE_3D_ID,
    build_eye_closure_prompt,
    build_prompt,
)


def test_prompt_contains_required_pieces():
    prompt = build_prompt(
        "an orange cat",
        {"fur_colors": "orange tabby", "eye_color": "green"},
    )
    for fragment in [
        "chibi",
        "front view",
        "sitting upright",
        "light grey background",
        "orange tabby",
        "green",
        "recognise",
    ]:
        assert fragment in prompt, f"missing: {fragment}"


def test_prompt_without_traits_is_clean():
    prompt = build_prompt("a cat", {})
    assert "Preserve these locked identity traits" not in prompt


def test_prompt_includes_all_constraint_fragments():
    prompt = build_prompt("a cat", {})
    for constraint in STYLE_CONSTRAINTS:
        core = constraint.rstrip(".")
        assert core.split(":")[0].split(" (")[0] in prompt


def test_3d_style_prompt_uses_3d_block_and_constraints():
    prompt = build_prompt("an orange cat", {"fur_colors": "orange"}, style=STYLE_3D_ID)
    for fragment in ["3D rendered", "clay-like fur", "front view", "light grey background"]:
        assert fragment in prompt, f"missing: {fragment}"
    for constraint in STYLE_3D_CONSTRAINTS:
        core = constraint.rstrip(".")
        assert core.split(":")[0].split(" (")[0] in prompt


def test_3d_style_still_keeps_identity_traits():
    prompt = build_prompt(
        "an orange cat",
        {"fur_colors": "orange tabby", "eye_color": "green"},
        style=STYLE_3D_ID,
    )
    assert "orange tabby" in prompt
    assert "green" in prompt


def test_unknown_style_rejected():
    import pytest

    with pytest.raises(ValueError):
        build_prompt("a cat", {}, style="unknown-style")


def test_eye_closure_prompt_requests_closed_eyes():
    prompt = build_eye_closure_prompt("a cat", {"eye_color": "green"})
    assert "eyes closed" in prompt
    assert "closed-eye lines" in prompt

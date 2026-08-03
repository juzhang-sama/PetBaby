# -*- coding: utf-8 -*-
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import pytest  # noqa: E402

from identity import LockedTraits  # noqa: E402


def test_valid_traits_round_trip():
    traits = LockedTraits.from_dict(
        {
            "species": "cat",
            "fur_colors": ["orange", "white"],
            "pattern": "tabby",
            "ears": "triangular",
            "eye_color": "green",
        }
    )
    block = traits.to_prompt_block()
    assert block["fur_colors"] == "orange, white"
    assert block["eye_color"] == "green"
    assert LockedTraits.from_dict(traits.to_dict()) == traits


def test_invalid_species_rejected():
    with pytest.raises(ValueError, match="species"):
        LockedTraits.from_dict({"species": "bird", "fur_colors": ["x"]})


def test_empty_fur_colors_allowed_for_reference_only():
    # empty fur_colors means "rely on the reference photo" - prompt block is empty
    traits = LockedTraits.from_dict({"species": "cat", "fur_colors": []})
    assert traits.to_prompt_block() == {}

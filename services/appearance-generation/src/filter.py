# -*- coding: utf-8 -*-
"""Candidate quality filtering."""
from dataclasses import dataclass

from PIL import Image

MIN_DIMENSION = 512


@dataclass
class FilterReport:
    kept: int
    rejected: list[tuple[int, str]]  # (index, reason)


def filter_candidates(candidates: list[Image.Image]) -> FilterReport:
    """Filter by transparency failure, minimum size and content integrity."""
    rejected: list[tuple[int, str]] = []
    kept_indices: list[int] = []
    for index, image in enumerate(candidates):
        if image is None:
            rejected.append((index, "missing"))
            continue
        if image.mode != "RGBA":
            rejected.append((index, "not-transparent"))
            continue
        if image.width < MIN_DIMENSION or image.height < MIN_DIMENSION:
            rejected.append((index, f"too-small-{image.width}x{image.height}"))
            continue
        if _is_blank(image):
            rejected.append((index, "blank-content"))
            continue
        kept_indices.append(index)
    return FilterReport(kept=len(kept_indices), rejected=rejected)


def _is_blank(image: Image.Image, threshold: float = 0.05) -> bool:
    """True when less than 5% of the pixels are opaque."""
    from PIL import Image as _I

    if image.mode != "RGBA":
        image = image.convert("RGBA")
    alpha = image.split()[3]
    histogram = alpha.histogram()
    opaque = sum(histogram[32:])
    total = alpha.width * alpha.height
    return total > 0 and opaque / total < threshold

# -*- coding: utf-8 -*-
"""Signature cartoon prompt builder (same contract as desktop creation-flow)."""


ANALYZED_TRAIT_LABELS = {
    "fur_colors": "main fur colors",
    "pattern": "coat pattern",
    "ears": "ear shape",
    "eye_color": "eye color",
    "face_notes": "face details",
}

STYLE_CARTOON = "cartoon"
STYLE_3D = "3d"

_STYLE_BLOCKS = {
    STYLE_CARTOON: (
        "Create a cute chibi cartoon style with a round face, unified soft outlines, "
        "big expressive eyes, short rounded body, sitting upright."
    ),
    STYLE_3D: (
        "Create a cute stylized 3D rendered pet, like a premium 3D animated film "
        "character, smooth rounded anatomy, soft clay-like fur texture, gentle studio "
        "lighting, subtle subsurface scattering, big expressive eyes."
    ),
}


def build_prompt(
    species: str,
    traits: dict | None = None,
    style: str = STYLE_CARTOON,
) -> str:
    if style not in _STYLE_BLOCKS:
        raise ValueError(f"unknown style: {style}")
    prompt = (
        f"{_STYLE_BLOCKS[style]} "
        f"Subject: a {species}. "
        "Front view, facing the viewer directly, full body visible, "
        "plain pure white background, no shadow, no gradient, no text, no watermark. "
        "High fidelity to the reference: keep the exact fur colors, markings, ear shape, "
        "eye color and face proportions so the owner can recognise the pet. "
        "Faithful face details: keep eye shape, eye colour and highlights, nose, whiskers, "
        "mouth and face markings; calm natural expression."
    )
    trait_lines = []
    for key, label in ANALYZED_TRAIT_LABELS.items():
        value = (traits or {}).get(key)
        if value:
            rendered = ", ".join(value) if isinstance(value, list) else str(value)
            if rendered:
                trait_lines.append(f"{label}: {rendered}")
    if trait_lines:
        prompt += (
            " Preserve these locked identity traits: "
            + "; ".join(trait_lines)
            + "."
        )
    return prompt


GUIDED_TRAIT_LABELS = {
    "body": "body shape",
    "fur": "fur length and texture",
    "color": "main coat color",
    "pattern": "coat pattern",
    "face": "face shape and expression",
    "accessory": "signature accessory",
}


def build_guided_prompt(
    species: str,
    traits: dict,
    style: str = STYLE_CARTOON,
) -> str:
    """Compose a prompt from user-selected part options (guided creation)."""
    if style not in _STYLE_BLOCKS:
        raise ValueError(f"unknown style: {style}")
    style_block = _STYLE_BLOCKS[style]
    parts = [
        style_block,
        f"Subject: a {species}.",
    ]
    trait_lines = []
    for key, label in GUIDED_TRAIT_LABELS.items():
        value = (traits or {}).get(key)
        if value and value != "none":
            trait_lines.append(f"{label}: {value}")
    if trait_lines:
        parts.append("Preserve these chosen traits: " + "; ".join(trait_lines) + ".")
    parts.append(
        "Sitting upright, front view, full body visible, "
        "plain pure white background, no shadow, no gradient, "
        "no text, no watermark, calm friendly expression."
    )
    return " ".join(parts)

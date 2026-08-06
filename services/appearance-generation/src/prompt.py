# -*- coding: utf-8 -*-
"""Signature cartoon style prompt templates (signature-cartoon-v1)."""

STYLE_ID = "signature-cartoon-v1"

STYLE_3D_ID = "3d-render-v1"

_STYLES = {
    STYLE_ID: {
        "block": (
            "a cute chibi cartoon style with a round face, unified soft outlines, "
            "big expressive eyes, short rounded body, standing sitting upright"
        ),
        "constraints": [
            "front view, facing the viewer directly",
            "sitting upright, full body visible",
            "plain uniform light grey background (important: uniform solid color for easy cutout)",
            "no text, no watermark, no extra objects",
            "high fidelity to the reference: keep the exact fur/coat colors, markings, "
            "ear shape, eye color and face proportions so the owner can recognise the pet",
            "faithful face details: keep the exact eye shape, eye colour and highlights, "
            "nose shape, whiskers, mouth and any face markings; the pet's expression should "
            "be calm and natural, as in the reference photo",
        ],
    },
    STYLE_3D_ID: {
        "block": (
            "a cute stylized 3D rendered pet, like a premium 3D animated film character, "
            "smooth rounded anatomy, soft clay-like fur texture, gentle studio lighting, "
            "subtle subsurface scattering, big expressive eyes"
        ),
        "constraints": [
            "front view, facing the viewer directly",
            "sitting upright, full body visible",
            "plain uniform light grey background (important: uniform solid color for easy cutout)",
            "no text, no watermark, no extra objects",
            "high fidelity to the reference: keep the exact fur/coat colors, markings, "
            "ear shape, eye color and face proportions so the owner can recognise the pet",
            "faithful face details: keep the exact eye shape, eye colour and highlights, "
            "nose shape, whiskers, mouth and any face markings; the pet's expression should "
            "be calm and natural, as in the reference photo",
        ],
    },
}

STYLE_BLOCK = _STYLES[STYLE_ID]["block"]
STYLE_CONSTRAINTS = list(_STYLES[STYLE_ID]["constraints"])
STYLE_3D_BLOCK = _STYLES[STYLE_3D_ID]["block"]
STYLE_3D_CONSTRAINTS = list(_STYLES[STYLE_3D_ID]["constraints"])

LOCKED_TRAIT_LABELS = {
    "species": "species",
    "fur_colors": "main fur colors",
    "pattern": "coat pattern",
    "ears": "ear shape",
    "eye_color": "eye color",
    "face_notes": "additional face features",
}


def build_prompt(
    subject_desc: str,
    locked_traits: dict[str, str],
    extra: str = "",
    style: str = STYLE_ID,
) -> str:
    """Compose the full generation prompt."""
    if style not in _STYLES:
        raise ValueError(f"unknown style: {style}")
    style_block = _STYLES[style]["block"]
    style_constraints = _STYLES[style]["constraints"]
    parts = [
        f"Create {style_block}.",
    ]
    if subject_desc:
        parts.append(f"Subject: {subject_desc}.")
    trait_lines = []
    for key, label in LOCKED_TRAIT_LABELS.items():
        value = (locked_traits or {}).get(key)
        if value:
            trait_lines.append(f"{label}: {value}")
    if trait_lines:
        parts.append("Preserve these locked identity traits: " + "; ".join(trait_lines) + ".")
    parts.extend(f"{c}." if not c.endswith(".") else c for c in style_constraints)
    if extra:
        parts.append(extra)
    return " ".join(parts)


def build_eye_closure_prompt(subject_desc: str, locked_traits: dict[str, str]) -> str:
    """Prompt for the eye-closed layer experiment."""
    base = build_prompt(subject_desc, locked_traits, "eyes closed, relaxed sleeping face")
    return base.replace(
        "big expressive eyes",
        "eyes closed with simple closed-eye lines",
        1,
    )

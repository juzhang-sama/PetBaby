# -*- coding: utf-8 -*-
"""Signature cartoon style prompt templates (signature-cartoon-v1)."""

STYLE_ID = "signature-cartoon-v1"

STYLE_BLOCK = (
    "a cute chibi cartoon style with a round face, unified soft outlines, "
    "big expressive eyes, short rounded body, standing sitting upright"
)

STYLE_CONSTRAINTS = [
    "front view, facing the viewer directly",
    "sitting upright, full body visible",
    "plain uniform light grey background (important: uniform solid color for easy cutout)",
    "no text, no watermark, no extra objects",
    "high fidelity to the reference: keep the exact fur/coat colors, markings, "
    "ear shape, eye color and face proportions so the owner can recognise the pet",
]

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
) -> str:
    """Compose the full generation prompt."""
    parts = [
        f"Create {STYLE_BLOCK}.",
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
    parts.extend(f"{c}." if not c.endswith(".") else c for c in STYLE_CONSTRAINTS)
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

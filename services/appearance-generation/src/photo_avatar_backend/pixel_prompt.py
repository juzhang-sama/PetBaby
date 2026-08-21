import json

from .contracts import PixelAppearanceProfile
from .pixel_audit import JsonValue
from .pixel_style import PixelStylePack


TRAIT_KEYS = (
    "faceShape", "faceProportions", "eyeShape", "eyeColor", "earShape",
    "primaryFurColor", "secondaryFurColor", "faceMarkings", "chestMarkings",
    "pawMarkings", "bodyMarkings", "tailShape", "tailMarkings", "signatureMarks",
    "temperament",
)


def profile_wire(profile: PixelAppearanceProfile) -> dict[str, JsonValue]:
    return {
        "schemaVersion": profile.schema_version,
        "species": profile.species,
        "styleProfileId": profile.style_profile_id,
        "traits": [
            {
                "key": trait.key,
                "value": trait.value,
                "source": trait.source,
                "evidencePhotoIds": list(trait.evidence_photo_ids),
            }
            for trait in profile.traits
        ],
        "completionSummary": list(profile.completion_summary),
    }


def analysis_prompt(style_profile_id: str) -> str:
    keys = ", ".join(TRAIT_KEYS)
    shape = json.dumps(
        {
            "schemaVersion": 1,
            "species": "cat",
            "styleProfileId": style_profile_id,
            "traits": [
                {
                    "key": "<one allowed key>",
                    "value": "<string>",
                    "source": "user",
                    "evidencePhotoIds": ["photo-1"],
                }
            ],
            "completionSummary": [],
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
    return (
        "Extract only traits directly visible in the cat photo; do not invent or complete missing traits. "
        f"Return exactly one JSON object with this shape: {shape}. "
        "The only allowed trait keys are: "
        + keys
        + ". Include one trait entry for each key that is clearly visible, and omit uncertain keys."
    )


def completion_prompt(
    observed: PixelAppearanceProfile,
    modification: str | None,
    missing: tuple[str, ...],
) -> str:
    context = {
        "observed": profile_wire(observed),
        "missingTraitKeys": list(missing),
        "modification": modification,
    }
    return (
        "Complete only the listed missing pixel identity traits using the photo and observed traits. "
        "Return exactly one JSON object with schemaVersion 1, species cat, styleProfileId "
        + observed.style_profile_id
        + ", a traits array containing exactly these keys with source ai-completed and empty "
        "evidencePhotoIds, and completionSummary equal to the missing keys. Missing keys and context: "
        + json.dumps(context, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    )


def profile_schema(
    keys: tuple[str, ...],
    source: str,
    style_profile_id: str,
) -> dict[str, JsonValue]:
    evidence: dict[str, JsonValue]
    if source == "user":
        evidence = {
            "type": "array",
            "items": {"type": "string"},
            "description": "Must contain exactly [\"photo-1\"] for traits observed in the photo.",
        }
    else:
        evidence = {
            "type": "array",
            "items": {"type": "string"},
            "description": "Must be an empty array for ai-completed traits.",
        }
    trait: dict[str, JsonValue] = {
        "type": "object",
        "additionalProperties": False,
        "required": ["key", "value", "source", "evidencePhotoIds"],
        "properties": {
            "key": {"type": "string", "enum": list(keys)},
            "value": {"type": "string"},
            "source": {"type": "string", "enum": [source]},
            "evidencePhotoIds": evidence,
        },
    }
    completion_summary: dict[str, JsonValue]
    if source == "user":
        completion_summary = {
            "type": "array",
            "items": {"type": "string"},
            "description": "Must be an empty array [].",
        }
    else:
        completion_summary = {
            "type": "array",
            "items": {"type": "string"},
            "description": f"Must list exactly these keys: {list(keys)}.",
        }
    return {
        "type": "object",
        "additionalProperties": False,
        "required": ["schemaVersion", "species", "styleProfileId", "traits", "completionSummary"],
        "properties": {
            "schemaVersion": {"type": "integer", "enum": [1]},
            "species": {"type": "string", "enum": ["cat"]},
            "styleProfileId": {"type": "string", "enum": [style_profile_id]},
            "traits": {"type": "array", "items": trait},
            "completionSummary": completion_summary,
        },
    }


def generation_prompt(style: PixelStylePack, identity_json: str) -> str:
    return (
        "[项目不可修改风格]\n"
        + style.prompt_contract
        + "\n[宠物身份 JSON]\n"
        + identity_json
        + "\n[输出合同]\n一张完整 RGBA PNG，透明背景，不含文字。"
        + "\n[禁止项]\n"
        + json.dumps(style.profile["forbidden"], ensure_ascii=False)
    )

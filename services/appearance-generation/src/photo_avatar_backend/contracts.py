"""Wire contracts shared by the desktop photo-avatar provider and backend."""

import base64
import binascii
import hashlib
from dataclasses import dataclass
from io import BytesIO
from typing import Any, Mapping

from PIL import Image

from .pixel_style import PIXEL_STYLE_V1_ID, SUPPORTED_PIXEL_STYLE_IDS


class ContractError(ValueError):
    """Raised when an untrusted provider wire payload violates the contract."""


_STEP_FIELDS = frozenset(
    {
        "sessionId",
        "revision",
        "providerSessionId",
        "step",
        "attempt",
        "consentVersion",
        "sourceImages",
        "profile",
        "bodyModuleContractSha256",
        "modification",
        "lockedTraits",
    }
)
_SOURCE_IMAGE_FIELDS = frozenset({"sourceId", "pngBase64", "sha256", "width", "height"})
_STEPS = frozenset({"analyzeIdentity", "completeAppearance", "renderTextureAtlas"})
_PIXEL_STEPS = frozenset({"analyzeIdentity", "generatePixelAvatar"})
_PIXEL_TRAIT_KEYS = frozenset(
    {
        "faceShape",
        "faceProportions",
        "eyeShape",
        "eyeColor",
        "earShape",
        "primaryFurColor",
        "secondaryFurColor",
        "faceMarkings",
        "chestMarkings",
        "pawMarkings",
        "bodyMarkings",
        "tailShape",
        "tailMarkings",
        "signatureMarks",
        "temperament",
    }
)
_ERROR_CODES = frozenset(
    {
        "invalidInput",
        "auth",
        "quota",
        "contentPolicy",
        "unsupported",
        "network",
        "timeout",
        "provider5xx",
        "temporaryUnavailable",
    }
)
_PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
_MIN_DIMENSION = 256
_MAX_DIMENSION = 4096
_MAX_PIXELS = 16_000_000


@dataclass(frozen=True)
class SourceImage:
    source_id: str
    png: bytes
    sha256: str
    width: int
    height: int


@dataclass(frozen=True)
class StepRequest:
    session_id: str
    revision: int
    provider_session_id: str | None
    step: str
    attempt: int
    consent_version: str
    source_images: tuple[SourceImage, ...]
    profile: dict[str, Any] | None
    body_module_contract_sha256: str | None
    modification: str | None
    locked_traits: tuple[str, ...]

    @classmethod
    def parse(cls, payload: Mapping[str, Any]) -> "StepRequest":
        if not isinstance(payload, Mapping):
            raise ContractError("request must be an object")
        _require_exact_fields(payload, _STEP_FIELDS)

        step = _require_string(payload, "step")
        if step not in _STEPS:
            raise ContractError(f"unsupported step: {step}")
        revision = _require_int(payload, "revision", minimum=0)
        attempt = _require_int(payload, "attempt", minimum=1, maximum=3)
        session_id = _require_string(payload, "sessionId")
        consent_version = _require_string(payload, "consentVersion")
        provider_session_id = _optional_string(payload, "providerSessionId")
        modification = _optional_string(payload, "modification")
        body_module_contract_sha256 = _optional_sha256(
            payload, "bodyModuleContractSha256"
        )
        profile = payload["profile"]
        if profile is not None and not isinstance(profile, dict):
            raise ContractError("profile must be an object or null")
        locked_traits = payload["lockedTraits"]
        if not isinstance(locked_traits, list) or not all(
            isinstance(trait, str) and trait.strip() for trait in locked_traits
        ):
            raise ContractError("lockedTraits must be an array of non-empty strings")

        raw_images = payload["sourceImages"]
        if not isinstance(raw_images, list):
            raise ContractError("sourceImages must be an array")
        if step in {"analyzeIdentity", "renderTextureAtlas"}:
            if not 1 <= len(raw_images) <= 8:
                raise ContractError("sourceImages count must be 1..8")
        elif raw_images:
            raise ContractError("completeAppearance sourceImages must be empty")
        if step != "analyzeIdentity" and provider_session_id is None:
            raise ContractError("providerSessionId is required for subsequent steps")

        return cls(
            session_id=session_id,
            revision=revision,
            provider_session_id=provider_session_id,
            step=step,
            attempt=attempt,
            consent_version=consent_version,
            source_images=tuple(_parse_source_image(image) for image in raw_images),
            profile=profile,
            body_module_contract_sha256=body_module_contract_sha256,
            modification=modification,
            locked_traits=tuple(locked_traits),
        )


@dataclass(frozen=True)
class PixelIdentityTrait:
    key: str
    value: str
    source: str
    evidence_photo_ids: tuple[str, ...]


@dataclass(frozen=True)
class PixelAppearanceProfile:
    schema_version: int
    species: str
    style_profile_id: str
    traits: tuple[PixelIdentityTrait, ...]
    completion_summary: tuple[str, ...]

    @classmethod
    def parse(cls, payload: Mapping[str, Any]) -> "PixelAppearanceProfile":
        if not isinstance(payload, Mapping):
            raise ContractError("pixel profile must be an object")
        _require_exact_fields(
            payload,
            frozenset({"schemaVersion", "species", "styleProfileId", "traits", "completionSummary"}),
        )
        if payload["schemaVersion"] != 1:
            raise ContractError("pixel profile schemaVersion must be 1")
        if payload["species"] != "cat":
            raise ContractError("pixel profile species must be cat")
        style_profile_id = _require_supported_pixel_style(payload["styleProfileId"])
        raw_traits = payload["traits"]
        raw_summary = payload["completionSummary"]
        if not isinstance(raw_traits, list) or not isinstance(raw_summary, list):
            raise ContractError("pixel profile traits and completionSummary must be arrays")
        summary = tuple(_pixel_trait_key(value, "completionSummary") for value in raw_summary)
        if len(set(summary)) != len(summary):
            raise ContractError("pixel profile completionSummary contains duplicates")
        traits = tuple(_parse_pixel_trait(value, index) for index, value in enumerate(raw_traits))
        keys = tuple(trait.key for trait in traits)
        if len(set(keys)) != len(keys):
            raise ContractError("pixel profile contains duplicate trait keys")
        completed_keys = tuple(trait.key for trait in traits if trait.source == "ai-completed")
        if set(summary) != set(completed_keys):
            raise ContractError("pixel completionSummary must equal completed trait keys")
        return cls(1, "cat", style_profile_id, traits, summary)


@dataclass(frozen=True)
class PixelStepRequest:
    style_profile_id: str
    session_id: str
    revision: int
    provider_session_id: str | None
    step: str
    attempt: int
    consent_version: str
    source_images: tuple[SourceImage, ...]
    profile: PixelAppearanceProfile | None
    modification: str | None
    locked_traits: tuple[str, ...]

    @classmethod
    def parse(cls, payload: Mapping[str, Any]) -> "PixelStepRequest":
        if not isinstance(payload, Mapping):
            raise ContractError("pixel request must be an object")
        legacy_fields = frozenset(
            {
                "route",
                "sessionId",
                "revision",
                "providerSessionId",
                "step",
                "attempt",
                "consentVersion",
                "sourceImages",
                "profile",
                "modification",
                "lockedTraits",
            }
        )
        fields = legacy_fields | {"styleProfileId"}
        payload_fields = frozenset(payload)
        if payload_fields == legacy_fields:
            style_profile_id = PIXEL_STYLE_V1_ID
        elif payload_fields == fields:
            style_profile_id = _require_supported_pixel_style(payload["styleProfileId"])
        else:
            _require_exact_fields(payload, fields)
            raise AssertionError("unreachable")
        if payload["route"] != "pixel-v1":
            raise ContractError("pixel request route must be pixel-v1")
        step = _require_string(payload, "step")
        if step not in _PIXEL_STEPS:
            raise ContractError(f"unsupported pixel step: {step}")
        revision = _require_int(payload, "revision", minimum=0)
        attempt = _require_int(payload, "attempt", minimum=1, maximum=3)
        session_id = _require_string(payload, "sessionId")
        consent_version = _require_string(payload, "consentVersion")
        provider_session_id = _optional_string(payload, "providerSessionId")
        modification = _optional_string(payload, "modification")
        raw_profile = payload["profile"]
        profile = None if raw_profile is None else PixelAppearanceProfile.parse(raw_profile)
        raw_locked = payload["lockedTraits"]
        if not isinstance(raw_locked, list):
            raise ContractError("lockedTraits must be an array")
        locked_traits = tuple(_pixel_trait_key(value, "lockedTraits") for value in raw_locked)
        if len(set(locked_traits)) != len(locked_traits):
            raise ContractError("lockedTraits contains duplicates")
        raw_images = payload["sourceImages"]
        if not isinstance(raw_images, list) or not 1 <= len(raw_images) <= 8:
            raise ContractError("pixel sourceImages count must be 1..8")
        if step == "analyzeIdentity" and profile is not None:
            raise ContractError("analyzeIdentity profile must be null")
        if step == "generatePixelAvatar" and profile is None:
            raise ContractError("generatePixelAvatar profile is required")
        if profile is not None and profile.style_profile_id != style_profile_id:
            raise ContractError("pixel profile styleProfileId does not match request")
        if step == "generatePixelAvatar" and provider_session_id is None:
            raise ContractError("generatePixelAvatar providerSessionId is required")
        return cls(
            style_profile_id=style_profile_id,
            session_id=session_id,
            revision=revision,
            provider_session_id=provider_session_id,
            step=step,
            attempt=attempt,
            consent_version=consent_version,
            source_images=tuple(_parse_source_image(image) for image in raw_images),
            profile=profile,
            modification=modification,
            locked_traits=locked_traits,
        )


def parse_step_request(payload: Mapping[str, Any]) -> StepRequest | PixelStepRequest:
    route = payload.get("route") if isinstance(payload, Mapping) else None
    if route == "pixel-v1":
        return PixelStepRequest.parse(payload)
    return StepRequest.parse(payload)


@dataclass(frozen=True)
class ProviderErrorPayload:
    code: str
    message: str

    def __post_init__(self) -> None:
        if self.code not in _ERROR_CODES:
            raise ContractError(f"unsupported error code: {self.code}")
        if not isinstance(self.message, str) or not self.message.strip():
            raise ContractError("error message must be a non-empty string")

    def to_wire(self) -> dict[str, str]:
        return {"code": self.code, "message": self.message}


def _require_exact_fields(payload: Mapping[str, Any], expected: frozenset[str]) -> None:
    for name in payload:
        if name not in expected:
            raise ContractError(f"unknown field: {name}")
    for name in expected:
        if name not in payload:
            raise ContractError(f"missing field: {name}")


def _require_supported_pixel_style(value: Any) -> str:
    if not isinstance(value, str) or value not in SUPPORTED_PIXEL_STYLE_IDS:
        raise ContractError("pixel styleProfileId is not supported")
    return value


def _require_string(payload: Mapping[str, Any], name: str) -> str:
    value = payload[name]
    if not isinstance(value, str) or not value.strip():
        raise ContractError(f"{name} must be a non-empty string")
    return value


def _optional_string(payload: Mapping[str, Any], name: str) -> str | None:
    value = payload[name]
    if value is None:
        return None
    if not isinstance(value, str) or not value.strip():
        raise ContractError(f"{name} must be a non-empty string or null")
    return value


def _require_int(
    payload: Mapping[str, Any], name: str, minimum: int, maximum: int | None = None
) -> int:
    value = payload[name]
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise ContractError(f"{name} is outside the allowed range")
    if maximum is not None and value > maximum:
        raise ContractError(f"{name} is outside the allowed range")
    return value


def _optional_sha256(payload: Mapping[str, Any], name: str) -> str | None:
    value = payload[name]
    if value is None:
        return None
    _validate_sha256(value, name)
    return value


def _pixel_trait_key(value: Any, label: str) -> str:
    if not isinstance(value, str) or value not in _PIXEL_TRAIT_KEYS:
        raise ContractError(f"pixel {label} contains an unknown trait key")
    return value


def _parse_pixel_trait(value: Any, index: int) -> PixelIdentityTrait:
    if not isinstance(value, Mapping):
        raise ContractError(f"pixel trait {index} must be an object")
    _require_exact_fields(
        value, frozenset({"key", "value", "source", "evidencePhotoIds"})
    )
    key = _pixel_trait_key(value["key"], f"trait {index}")
    trait_value = _require_string(value, "value")
    source = _require_string(value, "source")
    if source not in {"user", "ai-completed"}:
        raise ContractError(f"pixel trait {key} source is invalid")
    raw_evidence = value["evidencePhotoIds"]
    if not isinstance(raw_evidence, list) or not all(
        isinstance(photo_id, str) and photo_id.strip() for photo_id in raw_evidence
    ):
        raise ContractError(f"pixel trait {key} evidencePhotoIds are invalid")
    evidence = tuple(raw_evidence)
    if source == "user" and not evidence:
        raise ContractError(f"pixel trait {key} requires photo evidence")
    if source == "ai-completed" and evidence:
        raise ContractError(f"pixel trait {key} completion cannot claim photo evidence")
    return PixelIdentityTrait(key, trait_value, source, evidence)


def _parse_source_image(value: Any) -> SourceImage:
    if not isinstance(value, Mapping):
        raise ContractError("source image must be an object")
    for name in value:
        if name not in _SOURCE_IMAGE_FIELDS:
            raise ContractError(f"unknown source image field: {name}")
    for name in _SOURCE_IMAGE_FIELDS:
        if name not in value:
            raise ContractError(f"missing field: {name}")

    source_id = _require_string(value, "sourceId")
    declared_sha256 = _require_string(value, "sha256")
    _validate_sha256(declared_sha256, "sha256")
    encoded = _require_string(value, "pngBase64")
    try:
        png = base64.b64decode(encoded, validate=True)
    except (binascii.Error, ValueError) as exc:
        raise ContractError("pngBase64 must be valid base64") from exc
    if not png.startswith(_PNG_SIGNATURE):
        raise ContractError("source image must be PNG")
    width = _require_int(value, "width", _MIN_DIMENSION, _MAX_DIMENSION)
    height = _require_int(value, "height", _MIN_DIMENSION, _MAX_DIMENSION)
    if width * height > _MAX_PIXELS:
        raise ContractError("source image dimensions exceed the supported limit")
    try:
        with Image.open(BytesIO(png)) as image:
            if image.format != "PNG":
                raise ContractError("source image must be valid PNG")
            actual_width, actual_height = image.size
            image.verify()
    except ContractError:
        raise
    except (OSError, SyntaxError, ValueError) as exc:
        raise ContractError("source image must be valid PNG") from exc
    if (
        not _MIN_DIMENSION <= actual_width <= _MAX_DIMENSION
        or not _MIN_DIMENSION <= actual_height <= _MAX_DIMENSION
    ):
        raise ContractError("source image dimensions are outside the supported limit")
    if (width, height) != (actual_width, actual_height):
        raise ContractError("source image dimensions do not match PNG")
    if hashlib.sha256(png).hexdigest() != declared_sha256:
        raise ContractError("source image SHA-256 mismatch")
    return SourceImage(source_id, png, declared_sha256, width, height)


def _validate_sha256(value: Any, name: str) -> None:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise ContractError(f"{name} must be lowercase SHA-256")

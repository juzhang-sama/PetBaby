import base64
import hashlib
import struct
import sys
import zlib
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from photo_avatar_backend.contracts import (  # noqa: E402
    ContractError,
    PixelStepRequest,
    ProviderErrorPayload,
    StepRequest,
)
from photo_avatar_backend.pixel_audit import JsonValue  # noqa: E402


def png_bytes(width: int = 256, height: int = 256) -> bytes:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    pixels = b"\x00" + b"\x00\x00\x00\xff" * width
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(pixels * height))
        + chunk(b"IEND", b"")
    )


def valid_source_image() -> dict[str, object]:
    image = png_bytes()
    return {
        "sourceId": "source-0",
        "pngBase64": base64.b64encode(image).decode("ascii"),
        "sha256": hashlib.sha256(image).hexdigest(),
        "width": 256,
        "height": 256,
    }


def valid_step_request() -> dict[str, object]:
    return {
        "sessionId": "session-1",
        "revision": 0,
        "providerSessionId": None,
        "step": "analyzeIdentity",
        "attempt": 1,
        "consentVersion": "photo-avatar-third-party-ai-lk888-no-delete-v2",
        "sourceImages": [valid_source_image()],
        "profile": None,
        "bodyModuleContractSha256": None,
        "modification": None,
        "lockedTraits": [],
    }


def pixel_request_wire(*, include_style: bool = True) -> dict[str, JsonValue]:
    payload: dict[str, JsonValue] = {
        "route": "pixel-v1",
        "sessionId": "session-1",
        "revision": 0,
        "providerSessionId": None,
        "step": "analyzeIdentity",
        "attempt": 1,
        "consentVersion": "photo-avatar-third-party-ai-lk888-no-delete-v2",
        "sourceImages": [valid_source_image()],
        "profile": None,
        "modification": None,
        "lockedTraits": [],
    }
    if include_style:
        payload["styleProfileId"] = "pixel-style-v2-animation-ready"
    return payload


def test_pixel_request_accepts_supported_explicit_style() -> None:
    payload = pixel_request_wire()

    request = PixelStepRequest.parse(payload)

    assert request.style_profile_id == "pixel-style-v2-animation-ready"


def test_pixel_request_rejects_unknown_explicit_style() -> None:
    payload = pixel_request_wire()
    payload["styleProfileId"] = "pixel-style-v3"

    with pytest.raises(ContractError, match="styleProfileId"):
        PixelStepRequest.parse(payload)


def test_legacy_pixel_request_without_style_is_v1_only() -> None:
    payload = pixel_request_wire(include_style=False)

    request = PixelStepRequest.parse(payload)

    assert request.style_profile_id == "pixel-style-v1"


def test_pixel_request_rejects_profile_style_mismatch() -> None:
    payload = pixel_request_wire()
    payload.update(
        {
            "providerSessionId": "provider-1",
            "step": "generatePixelAvatar",
            "profile": {
                "schemaVersion": 1,
                "species": "cat",
                "styleProfileId": "pixel-style-v1",
                "traits": [],
                "completionSummary": [],
            },
        }
    )

    with pytest.raises(ContractError, match="profile styleProfileId"):
        PixelStepRequest.parse(payload)


def test_step_request_parses_the_frozen_desktop_wire_contract():
    request = StepRequest.parse(valid_step_request())

    assert request.session_id == "session-1"
    assert request.step == "analyzeIdentity"
    assert request.source_images[0].width == 256


def test_step_request_rejects_unknown_fields_and_more_than_eight_images():
    payload = valid_step_request()
    payload["extra"] = True
    with pytest.raises(ContractError, match="unknown field: extra"):
        StepRequest.parse(payload)

    payload = valid_step_request()
    payload["sourceImages"] = [valid_source_image() for _ in range(9)]
    with pytest.raises(ContractError, match="1..8"):
        StepRequest.parse(payload)


@pytest.mark.parametrize("step", ["analyzeIdentity", "renderTextureAtlas"])
def test_analysis_and_atlas_require_one_to_eight_source_images(step):
    payload = valid_step_request()
    payload.update(
        step=step,
        providerSessionId=None if step == "analyzeIdentity" else "provider-1",
        sourceImages=[],
    )
    with pytest.raises(ContractError, match="1..8"):
        StepRequest.parse(payload)

    payload["sourceImages"] = [valid_source_image() for _ in range(9)]
    with pytest.raises(ContractError, match="1..8"):
        StepRequest.parse(payload)


def test_complete_appearance_requires_empty_source_images():
    payload = valid_step_request()
    payload.update(
        step="completeAppearance",
        providerSessionId="provider-1",
        sourceImages=[valid_source_image()],
    )

    with pytest.raises(ContractError, match="completeAppearance"):
        StepRequest.parse(payload)


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("step", "render", "unsupported step"),
        ("revision", -1, "revision"),
        ("attempt", 0, "attempt"),
        ("attempt", 4, "attempt"),
        ("profile", [], "profile"),
        ("lockedTraits", ["face", 3], "lockedTraits"),
    ],
)
def test_step_request_rejects_invalid_scalar_and_object_fields(field, value, message):
    payload = valid_step_request()
    payload[field] = value

    with pytest.raises(ContractError, match=message):
        StepRequest.parse(payload)


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (lambda source: source.update(pngBase64="not-base64"), "base64"),
        (lambda source: source.update(pngBase64=base64.b64encode(b"not-png").decode("ascii")), "PNG"),
        (lambda source: source.update(sha256="A" * 64), "lowercase SHA-256"),
        (lambda source: source.update(width=255), "allowed range"),
        (lambda source: source.update(height=4097), "allowed range"),
        (lambda source: source.update(sha256="0" * 64), "SHA-256 mismatch"),
    ],
)
def test_step_request_rejects_invalid_source_image_content(mutate, message):
    payload = valid_step_request()
    source = payload["sourceImages"][0]
    mutate(source)

    with pytest.raises(ContractError, match=message):
        StepRequest.parse(payload)


def test_step_request_rejects_missing_or_extra_source_image_fields():
    payload = valid_step_request()
    payload["sourceImages"][0].pop("height")
    with pytest.raises(ContractError, match="missing field: height"):
        StepRequest.parse(payload)

    payload = valid_step_request()
    payload["sourceImages"][0]["extra"] = True
    with pytest.raises(ContractError, match="unknown source image field: extra"):
        StepRequest.parse(payload)


def test_step_request_rejects_truncated_png_even_when_hash_and_dimensions_match():
    payload = valid_step_request()
    truncated = png_bytes()[:-20]
    source = payload["sourceImages"][0]
    source["pngBase64"] = base64.b64encode(truncated).decode("ascii")
    source["sha256"] = hashlib.sha256(truncated).hexdigest()

    with pytest.raises(ContractError, match="valid PNG"):
        StepRequest.parse(payload)


def test_provider_error_payload_only_emits_existing_error_taxonomy():
    payload = ProviderErrorPayload(code="temporaryUnavailable", message="try again")

    assert payload.to_wire() == {
        "code": "temporaryUnavailable",
        "message": "try again",
    }
    with pytest.raises(ContractError, match="unsupported error code"):
        ProviderErrorPayload(code="madeUp", message="nope")

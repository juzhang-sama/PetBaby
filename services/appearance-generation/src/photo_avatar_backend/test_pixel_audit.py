from collections.abc import Mapping

import pytest

from .pixel_audit import (
    JsonValue,
    PixelAuditError,
    PixelAvatarAuditV1,
    PixelAvatarAuditV2,
    parse_pixel_avatar_audit,
)


def _alpha_wire() -> Mapping[str, JsonValue]:
    return {
        "visiblePixels": 640000,
        "partialAlphaPixels": 0,
        "partialAlphaRatio": 0.0,
        "largestComponentPixels": 640000,
        "largestComponentShare": 1.0,
        "boundsLeft": 112,
        "boundsTop": 112,
        "boundsRight": 912,
        "boundsBottom": 912,
        "marginLeft": 112,
        "marginTop": 112,
        "marginRight": 112,
        "marginBottom": 112,
    }


def _common_wire(style_profile_id: str) -> dict[str, JsonValue]:
    style_profile_sha256, reference_sha256, prompt_template_version = {
        "pixel-style-v1": (
            "342d61eaf88eecba41bbb7a21c76c000aa16d6b86dce03ef570431f746e34830",
            "5ebbaece6553ffa450731660aa0d3fbb208d8f2761e48eabfe696bc20a39447a",
            "pixel-style-v1-prompt-v1",
        ),
        "pixel-style-v2-animation-ready": (
            "2a48f382d0d0a579010ffae2ce90a7693d364a0cf64e5463e0ce7bf0291ee4ab",
            "75171817d27aee72439f373317ad0a3f43bdb2f8a76b0f8c55e24c306ac46c85",
            "pixel-style-v2-animation-ready-prompt-v2",
        ),
    }[style_profile_id]
    return {
        "schemaVersion": 1,
        "sessionId": "session-a",
        "revision": 2,
        "attempt": 1,
        "provider": "lk888",
        "providerModel": "gpt-image-2",
        "providerTaskId": "111247514",
        "styleProfileId": style_profile_id,
        "styleProfileSha256": style_profile_sha256,
        "referenceSha256": reference_sha256,
        "promptTemplateVersion": prompt_template_version,
        "identityProfileSha256": "3" * 64,
        "providerRawSha256": "4" * 64,
        "normalizedSha256": "5" * 64,
        "width": 1024,
        "height": 1024,
        "alphaReport": _alpha_wire(),
        "privacyPolicyVersion": "privacy-v1",
        "retentionPolicy": "local-only",
        "upstreamDeleteApi": "unsupported",
        "status": "succeeded",
        "errorCode": None,
        "createdAt": "2026-08-21T00:00:00+00:00",
        "completedAt": "2026-08-21T00:01:00+00:00",
    }


def _v1_wire() -> Mapping[str, JsonValue]:
    return _common_wire("pixel-style-v1")


def _v2_wire() -> Mapping[str, JsonValue]:
    wire = _common_wire("pixel-style-v2-animation-ready")
    wire["schemaVersion"] = 2
    wire.update(
        {
            "logicalGridSize": 160,
            "paletteColorLimit": 24,
            "visibleColorCount": 22,
            "quantizeMethod": "maxcoverage",
            "dither": "none",
            "protectedAccentSlots": 4,
            "protectedAccentCount": 2,
            "downsample": "box",
            "upsample": "nearest",
        }
    )
    return wire


def test_parser_reads_historical_v1_and_current_v2_audits() -> None:
    # Given
    historical_wire = _v1_wire()
    current_wire = _v2_wire()

    # When
    historical = parse_pixel_avatar_audit(historical_wire)
    current = parse_pixel_avatar_audit(current_wire)

    # Then
    assert type(historical) is PixelAvatarAuditV1
    assert type(current) is PixelAvatarAuditV2
    assert current.visible_color_count == 22
    assert current.protected_accent_count == 2


def test_v2_audit_round_trips_the_exact_wire_contract() -> None:
    # Given
    wire = _v2_wire()

    # When
    parsed = parse_pixel_avatar_audit(wire)

    # Then
    assert parsed.to_wire() == wire


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("logicalGridSize", 512),
        ("paletteColorLimit", 25),
        ("visibleColorCount", 0),
        ("visibleColorCount", 25),
        ("quantizeMethod", "mediancut"),
        ("dither", "floydsteinberg"),
        ("protectedAccentSlots", 3),
        ("protectedAccentCount", 5),
        ("downsample", "nearest"),
        ("upsample", "box"),
    ],
)
def test_v2_audit_rejects_noncanonical_postprocess_fields(
    field: str,
    value: JsonValue,
) -> None:
    # Given
    wire = dict(_v2_wire())
    wire[field] = value

    # When / Then
    with pytest.raises(PixelAuditError):
        parse_pixel_avatar_audit(wire)


def test_v2_audit_rejects_unknown_fields() -> None:
    # Given
    wire = dict(_v2_wire())
    wire["unexpected"] = True

    # When / Then
    with pytest.raises(PixelAuditError, match="fields"):
        parse_pixel_avatar_audit(wire)

from dataclasses import dataclass

from .pixel_audit_types import (
    JsonValue,
    PixelAlphaReportV1,
    PixelAuditError,
    audit_integer,
    audit_mapping,
    audit_sha,
    audit_text,
    validate_alpha_bounds,
)


_AUDIT_V2_FIELDS = frozenset(
    {
        "schemaVersion", "sessionId", "revision", "attempt", "provider",
        "providerModel", "providerTaskId", "styleProfileId", "styleProfileSha256",
        "referenceSha256", "promptTemplateVersion", "identityProfileSha256",
        "providerRawSha256", "normalizedSha256", "width", "height", "alphaReport",
        "privacyPolicyVersion", "retentionPolicy", "upstreamDeleteApi", "status",
        "errorCode", "createdAt", "completedAt", "logicalGridSize",
        "paletteColorLimit", "visibleColorCount", "quantizeMethod", "dither",
        "protectedAccentSlots", "protectedAccentCount", "downsample", "upsample",
    }
)


@dataclass(frozen=True, slots=True)
class PixelAvatarAuditV2:
    schema_version: int
    session_id: str
    revision: int
    attempt: int
    provider: str
    provider_model: str
    provider_task_id: str
    style_profile_id: str
    style_profile_sha256: str
    reference_sha256: str
    prompt_template_version: str
    identity_profile_sha256: str
    provider_raw_sha256: str
    normalized_sha256: str
    width: int
    height: int
    alpha_report: PixelAlphaReportV1
    privacy_policy_version: str
    retention_policy: str
    upstream_delete_api: str
    status: str
    error_code: str | None
    created_at: str
    completed_at: str
    logical_grid_size: int
    palette_color_limit: int
    visible_color_count: int
    quantize_method: str
    dither: str
    protected_accent_slots: int
    protected_accent_count: int
    downsample: str
    upsample: str

    def to_wire(self) -> dict[str, JsonValue]:
        wire: dict[str, JsonValue] = {
            "schemaVersion": self.schema_version,
            "sessionId": self.session_id,
            "revision": self.revision,
            "attempt": self.attempt,
            "provider": self.provider,
            "providerModel": self.provider_model,
            "providerTaskId": self.provider_task_id,
            "styleProfileId": self.style_profile_id,
            "styleProfileSha256": self.style_profile_sha256,
            "referenceSha256": self.reference_sha256,
            "promptTemplateVersion": self.prompt_template_version,
            "identityProfileSha256": self.identity_profile_sha256,
            "providerRawSha256": self.provider_raw_sha256,
            "normalizedSha256": self.normalized_sha256,
            "width": self.width,
            "height": self.height,
            "alphaReport": self.alpha_report.to_wire(),
            "privacyPolicyVersion": self.privacy_policy_version,
            "retentionPolicy": self.retention_policy,
            "upstreamDeleteApi": self.upstream_delete_api,
            "status": self.status,
            "errorCode": self.error_code,
            "createdAt": self.created_at,
            "completedAt": self.completed_at,
            "logicalGridSize": self.logical_grid_size,
            "paletteColorLimit": self.palette_color_limit,
            "visibleColorCount": self.visible_color_count,
            "quantizeMethod": self.quantize_method,
            "dither": self.dither,
            "protectedAccentSlots": self.protected_accent_slots,
            "protectedAccentCount": self.protected_accent_count,
            "downsample": self.downsample,
            "upsample": self.upsample,
        }
        self.from_wire(wire)
        return wire

    @classmethod
    def from_wire(cls, raw: JsonValue) -> "PixelAvatarAuditV2":
        value = audit_mapping(raw, _AUDIT_V2_FIELDS, "pixel avatar audit v2")
        fixed: dict[str, JsonValue] = {
            "schemaVersion": 2,
            "provider": "lk888",
            "providerModel": "gpt-image-2",
            "styleProfileId": "pixel-style-v2-animation-ready",
            "styleProfileSha256": "2a48f382d0d0a579010ffae2ce90a7693d364a0cf64e5463e0ce7bf0291ee4ab",
            "referenceSha256": "75171817d27aee72439f373317ad0a3f43bdb2f8a76b0f8c55e24c306ac46c85",
            "promptTemplateVersion": "pixel-style-v2-animation-ready-prompt-v2",
            "upstreamDeleteApi": "unsupported",
            "status": "succeeded",
            "errorCode": None,
            "width": 1024,
            "height": 1024,
            "logicalGridSize": 160,
            "paletteColorLimit": 24,
            "quantizeMethod": "maxcoverage",
            "dither": "none",
            "protectedAccentSlots": 4,
            "downsample": "box",
            "upsample": "nearest",
        }
        if any(value[key] != expected for key, expected in fixed.items()):
            raise PixelAuditError("pixel avatar audit v2 fixed metadata is invalid")
        alpha = PixelAlphaReportV1.from_wire(value["alphaReport"])
        validate_alpha_bounds(alpha, 1024, 1024)
        task_id = audit_text(value["providerTaskId"], "provider task id")
        if not task_id.isdigit():
            raise PixelAuditError("provider task id must be numeric")
        return cls(
            schema_version=2,
            session_id=audit_text(value["sessionId"], "session id"),
            revision=audit_integer(value["revision"], "revision", minimum=0),
            attempt=audit_integer(value["attempt"], "attempt", minimum=1, maximum=3),
            provider="lk888",
            provider_model="gpt-image-2",
            provider_task_id=task_id,
            style_profile_id="pixel-style-v2-animation-ready",
            style_profile_sha256=audit_sha(value["styleProfileSha256"]),
            reference_sha256=audit_sha(value["referenceSha256"]),
            prompt_template_version=audit_text(
                value["promptTemplateVersion"], "prompt template version"
            ),
            identity_profile_sha256=audit_sha(value["identityProfileSha256"]),
            provider_raw_sha256=audit_sha(value["providerRawSha256"]),
            normalized_sha256=audit_sha(value["normalizedSha256"]),
            width=1024,
            height=1024,
            alpha_report=alpha,
            privacy_policy_version=audit_text(
                value["privacyPolicyVersion"], "privacy policy version"
            ),
            retention_policy=audit_text(value["retentionPolicy"], "retention policy"),
            upstream_delete_api="unsupported",
            status="succeeded",
            error_code=None,
            created_at=audit_text(value["createdAt"], "createdAt"),
            completed_at=audit_text(value["completedAt"], "completedAt"),
            logical_grid_size=160,
            palette_color_limit=24,
            visible_color_count=audit_integer(
                value["visibleColorCount"], "visible color count", minimum=1, maximum=24
            ),
            quantize_method="maxcoverage",
            dither="none",
            protected_accent_slots=4,
            protected_accent_count=audit_integer(
                value["protectedAccentCount"],
                "protected accent count",
                minimum=0,
                maximum=4,
            ),
            downsample="box",
            upsample="nearest",
        )

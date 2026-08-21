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


_AUDIT_V1_FIELDS = frozenset(
    {
        "schemaVersion", "sessionId", "revision", "attempt", "provider",
        "providerModel", "providerTaskId", "styleProfileId", "styleProfileSha256",
        "referenceSha256", "promptTemplateVersion", "identityProfileSha256",
        "providerRawSha256", "normalizedSha256", "width", "height", "alphaReport",
        "privacyPolicyVersion", "retentionPolicy", "upstreamDeleteApi", "status",
        "errorCode", "createdAt", "completedAt",
    }
)


@dataclass(frozen=True, slots=True)
class PixelAvatarAuditV1:
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
        }
        self.from_wire(wire)
        return wire

    @classmethod
    def from_wire(cls, raw: JsonValue) -> "PixelAvatarAuditV1":
        value = audit_mapping(raw, _AUDIT_V1_FIELDS, "pixel avatar audit")
        fixed: dict[str, JsonValue] = {
            "schemaVersion": 1,
            "provider": "lk888",
            "providerModel": "gpt-image-2",
            "styleProfileId": "pixel-style-v1",
            "styleProfileSha256": "342d61eaf88eecba41bbb7a21c76c000aa16d6b86dce03ef570431f746e34830",
            "referenceSha256": "5ebbaece6553ffa450731660aa0d3fbb208d8f2761e48eabfe696bc20a39447a",
            "promptTemplateVersion": "pixel-style-v1-prompt-v1",
            "upstreamDeleteApi": "unsupported",
            "status": "succeeded",
            "errorCode": None,
        }
        if any(value[key] != expected for key, expected in fixed.items()):
            raise PixelAuditError("pixel avatar audit fixed metadata is invalid")
        width = audit_integer(value["width"], "width", minimum=1024, maximum=2048)
        height = audit_integer(value["height"], "height", minimum=1024, maximum=2048)
        if width * height > 4_194_304:
            raise PixelAuditError("pixel avatar audit dimensions exceed the pixel limit")
        alpha = PixelAlphaReportV1.from_wire(value["alphaReport"])
        validate_alpha_bounds(alpha, width, height)
        task_id = audit_text(value["providerTaskId"], "provider task id")
        if not task_id.isdigit():
            raise PixelAuditError("provider task id must be numeric")
        return cls(
            schema_version=1,
            session_id=audit_text(value["sessionId"], "session id"),
            revision=audit_integer(value["revision"], "revision", minimum=0),
            attempt=audit_integer(value["attempt"], "attempt", minimum=1, maximum=3),
            provider="lk888",
            provider_model="gpt-image-2",
            provider_task_id=task_id,
            style_profile_id="pixel-style-v1",
            style_profile_sha256=audit_sha(value["styleProfileSha256"]),
            reference_sha256=audit_sha(value["referenceSha256"]),
            prompt_template_version=audit_text(
                value["promptTemplateVersion"], "prompt template version"
            ),
            identity_profile_sha256=audit_sha(value["identityProfileSha256"]),
            provider_raw_sha256=audit_sha(value["providerRawSha256"]),
            normalized_sha256=audit_sha(value["normalizedSha256"]),
            width=width,
            height=height,
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
        )

"""Strict, photo-free audit contracts for photo-avatar provider attempts."""

from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass
import hashlib
from typing import Mapping

from .semantic_layers import SEMANTIC_LAYER_IDS


AUDIT_SCHEMA_VERSION = 1
API_CONTRACT_VERSION = "lk888-media-generate-v1"
PRIVACY_POLICY_VERSION = "unverified"
RETENTION_POLICY = "unverified"
UPSTREAM_DELETE_API = "unsupported"
_PROVIDER = "lk888"
_MODEL_DISPLAY_NAME = "GPT-image-2.0"
_MODELS = frozenset({"gpt-4o", "gpt-image-2"})
_BODY_MODULES = frozenset(
    {"body-slender-v1", "body-balanced-v1", "body-rounded-v1"}
)
_STATUSES = frozenset({"succeeded", "failed", "cancelled"})
_CONTEXT_FIELDS = frozenset(
    {
        "providerModel",
        "bodyModuleId",
        "moduleContractSha256",
        "sourceTextureSha256",
        "sourceAlphaSha256",
        "workCanvasSha256",
        "regionMapSha256",
        "composerVersion",
        "pngEncoderVersion",
    }
)
_AUDIT_FIELDS = frozenset(
    {
        "schemaVersion",
        "sessionId",
        "revision",
        "attempt",
        "provider",
        "providerModel",
        "modelDisplayName",
        "apiContractVersion",
        "privacyPolicyVersion",
        "retentionPolicy",
        "upstreamDeleteApi",
        "providerTaskId",
        "providerRawSha256",
        "canonicalSha256",
        "bodyModuleId",
        "moduleContractSha256",
        "sourceTextureSha256",
        "sourceAlphaSha256",
        "workCanvasSha256",
        "regionMapSha256",
        "composerVersion",
        "pngEncoderVersion",
        "coverageReport",
        "status",
        "errorCode",
        "createdAt",
        "completedAt",
    }
)


class AuditContractError(ValueError):
    """Raised when persisted or remote audit data is not canonical."""


@dataclass(frozen=True)
class SemanticLayerAuditV1:
    layer_id: str
    provider_raw_sha256: str
    canonical_layer_sha256: str
    mask_sha256: str
    attempt: int

    def to_wire(self) -> dict[str, object]:
        return {
            "layerId": self.layer_id,
            "providerRawSha256": self.provider_raw_sha256,
            "canonicalLayerSha256": self.canonical_layer_sha256,
            "maskSha256": self.mask_sha256,
            "attempt": self.attempt,
        }

    @classmethod
    def from_wire(cls, raw: object) -> "SemanticLayerAuditV1":
        value = _mapping(
            raw,
            frozenset(
                {
                    "layerId",
                    "providerRawSha256",
                    "canonicalLayerSha256",
                    "maskSha256",
                    "attempt",
                }
            ),
            "semantic layer audit",
        )
        layer_id = _text(value["layerId"], "semantic layer ID")
        if layer_id not in SEMANTIC_LAYER_IDS:
            raise AuditContractError("semantic layer audit ID is invalid")
        return cls(
            layer_id=layer_id,
            provider_raw_sha256=_required_sha(value["providerRawSha256"]),
            canonical_layer_sha256=_required_sha(value["canonicalLayerSha256"]),
            mask_sha256=_required_sha(value["maskSha256"]),
            attempt=_integer(value["attempt"], "layer attempt", minimum=1, maximum=3),
        )


@dataclass(frozen=True)
class SemanticAtlasAuditV1:
    identity_reference_sha256: str
    profile_sha256: str
    layers: tuple[SemanticLayerAuditV1, ...]
    canonical_atlas_sha256: str
    body_module_id: str

    def to_wire(self) -> dict[str, object]:
        wire = {
            "identityReferenceSha256": self.identity_reference_sha256,
            "profileSha256": self.profile_sha256,
            "layers": [layer.to_wire() for layer in self.layers],
            "canonicalAtlasSha256": self.canonical_atlas_sha256,
            "bodyModuleId": self.body_module_id,
        }
        self.from_wire(wire)
        return wire

    @classmethod
    def from_wire(cls, raw: object) -> "SemanticAtlasAuditV1":
        value = _mapping(
            raw,
            frozenset(
                {
                    "identityReferenceSha256",
                    "profileSha256",
                    "layers",
                    "canonicalAtlasSha256",
                    "bodyModuleId",
                }
            ),
            "semantic atlas audit",
        )
        raw_layers = value["layers"]
        if not isinstance(raw_layers, list):
            raise AuditContractError("semantic atlas audit layers are invalid")
        layers = tuple(SemanticLayerAuditV1.from_wire(layer) for layer in raw_layers)
        if tuple(layer.layer_id for layer in layers) != SEMANTIC_LAYER_IDS:
            raise AuditContractError("semantic atlas audit layer set is invalid")
        body_module_id = _module(value["bodyModuleId"])
        if body_module_id is None:
            raise AuditContractError("semantic atlas audit body module is invalid")
        return cls(
            identity_reference_sha256=_required_sha(value["identityReferenceSha256"]),
            profile_sha256=_required_sha(value["profileSha256"]),
            layers=layers,
            canonical_atlas_sha256=_required_sha(value["canonicalAtlasSha256"]),
            body_module_id=body_module_id,
        )

    def immutable_digest(self) -> str:
        fields = [self.identity_reference_sha256, self.profile_sha256]
        for layer in self.layers:
            fields.extend(
                (
                    layer.layer_id,
                    layer.provider_raw_sha256,
                    layer.canonical_layer_sha256,
                    layer.mask_sha256,
                    str(layer.attempt),
                )
            )
        fields.append(self.canonical_atlas_sha256)
        fields.append(self.body_module_id)
        return hashlib.sha256("\n".join(fields).encode("ascii")).hexdigest()


@dataclass(frozen=True)
class AuditContextV1:
    provider_model: str
    body_module_id: str | None = None
    module_contract_sha256: str | None = None
    source_texture_sha256: str | None = None
    source_alpha_sha256: str | None = None
    work_canvas_sha256: str | None = None
    region_map_sha256: str | None = None
    composer_version: str | None = None
    png_encoder_version: str | None = None

    def to_state(self) -> dict[str, object]:
        state = {
            "providerModel": self.provider_model,
            "bodyModuleId": self.body_module_id,
            "moduleContractSha256": self.module_contract_sha256,
            "sourceTextureSha256": self.source_texture_sha256,
            "sourceAlphaSha256": self.source_alpha_sha256,
            "workCanvasSha256": self.work_canvas_sha256,
            "regionMapSha256": self.region_map_sha256,
            "composerVersion": self.composer_version,
            "pngEncoderVersion": self.png_encoder_version,
        }
        self.from_state(state)
        return state

    @classmethod
    def from_state(cls, raw: object) -> "AuditContextV1":
        value = _mapping(raw, _CONTEXT_FIELDS, "audit context")
        context = cls(
            provider_model=_model(value["providerModel"]),
            body_module_id=_module(value["bodyModuleId"]),
            module_contract_sha256=_sha(value["moduleContractSha256"]),
            source_texture_sha256=_sha(value["sourceTextureSha256"]),
            source_alpha_sha256=_sha(value["sourceAlphaSha256"]),
            work_canvas_sha256=_sha(value["workCanvasSha256"]),
            region_map_sha256=_sha(value["regionMapSha256"]),
            composer_version=_optional_text(value["composerVersion"]),
            png_encoder_version=_optional_text(value["pngEncoderVersion"]),
        )
        if context.body_module_id is None and any(
            value is not None
            for value in (
                context.module_contract_sha256,
                context.source_texture_sha256,
                context.source_alpha_sha256,
                context.work_canvas_sha256,
                context.region_map_sha256,
                context.composer_version,
                context.png_encoder_version,
            )
        ):
            raise AuditContractError("audit context module fields are incomplete")
        return context


@dataclass(frozen=True)
class AttemptAuditV1:
    session_id: str
    revision: int
    attempt: int
    provider_task_id: str | None
    provider_model: str
    provider_raw_sha256: str | None
    canonical_sha256: str | None
    body_module_id: str | None
    module_contract_sha256: str | None
    source_texture_sha256: str | None
    source_alpha_sha256: str | None
    work_canvas_sha256: str | None
    region_map_sha256: str | None
    composer_version: str | None
    png_encoder_version: str | None
    coverage_report: dict[str, object] | None
    status: str
    error_code: str | None
    created_at: str
    completed_at: str

    def to_wire(self) -> dict[str, object]:
        wire = {
            "schemaVersion": AUDIT_SCHEMA_VERSION,
            "sessionId": self.session_id,
            "revision": self.revision,
            "attempt": self.attempt,
            "provider": _PROVIDER,
            "providerModel": self.provider_model,
            "modelDisplayName": _MODEL_DISPLAY_NAME,
            "apiContractVersion": API_CONTRACT_VERSION,
            "privacyPolicyVersion": PRIVACY_POLICY_VERSION,
            "retentionPolicy": RETENTION_POLICY,
            "upstreamDeleteApi": UPSTREAM_DELETE_API,
            "providerTaskId": self.provider_task_id,
            "providerRawSha256": self.provider_raw_sha256,
            "canonicalSha256": self.canonical_sha256,
            "bodyModuleId": self.body_module_id,
            "moduleContractSha256": self.module_contract_sha256,
            "sourceTextureSha256": self.source_texture_sha256,
            "sourceAlphaSha256": self.source_alpha_sha256,
            "workCanvasSha256": self.work_canvas_sha256,
            "regionMapSha256": self.region_map_sha256,
            "composerVersion": self.composer_version,
            "pngEncoderVersion": self.png_encoder_version,
            "coverageReport": deepcopy(self.coverage_report),
            "status": self.status,
            "errorCode": self.error_code,
            "createdAt": self.created_at,
            "completedAt": self.completed_at,
        }
        self.from_wire(wire)
        return wire

    @classmethod
    def from_wire(cls, raw: object) -> "AttemptAuditV1":
        value = _mapping(raw, _AUDIT_FIELDS, "attempt audit")
        if value["schemaVersion"] != AUDIT_SCHEMA_VERSION:
            raise AuditContractError("attempt audit schema is invalid")
        fixed = {
            "provider": _PROVIDER,
            "modelDisplayName": _MODEL_DISPLAY_NAME,
            "apiContractVersion": API_CONTRACT_VERSION,
            "privacyPolicyVersion": PRIVACY_POLICY_VERSION,
            "retentionPolicy": RETENTION_POLICY,
            "upstreamDeleteApi": UPSTREAM_DELETE_API,
        }
        if any(value[key] != expected for key, expected in fixed.items()):
            raise AuditContractError("attempt audit fixed metadata is invalid")
        revision = _integer(value["revision"], "revision", minimum=0)
        attempt = _integer(value["attempt"], "attempt", minimum=1, maximum=3)
        status = _text(value["status"], "status")
        if status not in _STATUSES:
            raise AuditContractError("attempt audit status is invalid")
        error_code = _optional_text(value["errorCode"])
        if (status == "failed") != (error_code is not None):
            raise AuditContractError("attempt audit error does not match status")
        coverage = value["coverageReport"]
        if coverage is not None and not isinstance(coverage, dict):
            raise AuditContractError("attempt audit coverage is invalid")
        if isinstance(coverage, dict) and "layers" in coverage:
            SemanticAtlasAuditV1.from_wire(coverage)
        return cls(
            session_id=_text(value["sessionId"], "session id"),
            revision=revision,
            attempt=attempt,
            provider_task_id=_optional_text(value["providerTaskId"]),
            provider_model=_model(value["providerModel"]),
            provider_raw_sha256=_sha(value["providerRawSha256"]),
            canonical_sha256=_sha(value["canonicalSha256"]),
            body_module_id=_module(value["bodyModuleId"]),
            module_contract_sha256=_sha(value["moduleContractSha256"]),
            source_texture_sha256=_sha(value["sourceTextureSha256"]),
            source_alpha_sha256=_sha(value["sourceAlphaSha256"]),
            work_canvas_sha256=_sha(value["workCanvasSha256"]),
            region_map_sha256=_sha(value["regionMapSha256"]),
            composer_version=_optional_text(value["composerVersion"]),
            png_encoder_version=_optional_text(value["pngEncoderVersion"]),
            coverage_report=deepcopy(coverage),
            status=status,
            error_code=error_code,
            created_at=_text(value["createdAt"], "createdAt"),
            completed_at=_text(value["completedAt"], "completedAt"),
        )


def _mapping(raw: object, fields: frozenset[str], label: str) -> Mapping[str, object]:
    if not isinstance(raw, Mapping) or set(raw) != fields:
        raise AuditContractError(f"{label} fields are invalid")
    return raw


def _text(value: object, label: str) -> str:
    if not isinstance(value, str) or not value or len(value) > 256:
        raise AuditContractError(f"{label} is invalid")
    return value


def _optional_text(value: object) -> str | None:
    return None if value is None else _text(value, "optional text")


def _integer(
    value: object, label: str, *, minimum: int, maximum: int | None = None
) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise AuditContractError(f"{label} is invalid")
    if maximum is not None and value > maximum:
        raise AuditContractError(f"{label} is invalid")
    return value


def _sha(value: object) -> str | None:
    if value is None:
        return None
    text = _text(value, "sha256")
    if len(text) != 64 or any(character not in "0123456789abcdef" for character in text):
        raise AuditContractError("sha256 is invalid")
    return text


def _required_sha(value: object) -> str:
    result = _sha(value)
    if result is None:
        raise AuditContractError("sha256 is required")
    return result


def _model(value: object) -> str:
    model = _text(value, "provider model")
    if model not in _MODELS:
        raise AuditContractError("provider model is invalid")
    return model


def _module(value: object) -> str | None:
    if value is None:
        return None
    module = _text(value, "body module")
    if module not in _BODY_MODULES:
        raise AuditContractError("body module is invalid")
    return module

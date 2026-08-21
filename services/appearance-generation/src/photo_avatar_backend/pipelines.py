"""Strict identity-analysis and appearance-completion pipelines.

The provider is deliberately passed into each operation.  The functions in
this module validate both sides of the provider boundary because the response
is untrusted input, even when it came from an injected test client.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
import hashlib
from io import BytesIO
import json
from pathlib import Path
import time
from typing import Any

from PIL import Image

from .audit import SemanticAtlasAuditV1, SemanticLayerAuditV1
from .contracts import ContractError, SourceImage, StepRequest
from .lk888_client import Lk888Error
from .semantic_layers import (
    SEMANTIC_LAYER_IDS,
    SemanticLayerSpec,
    ValidatedSemanticLayer,
    build_semantic_layer_specs,
    validate_semantic_layer_png,
)
from .texture_compositor import (
    COMPOSER_VERSION,
    PNG_ENCODER_VERSION,
    compose_semantic_atlas,
)
from .uv_guides import (
    BODY_MODULE_IDS,
    GENERATOR_VERSION,
    build_module_semantic_snapshot,
    build_work_canvas,
    resolve_module_file,
)


TRAIT_KEYS = (
    "faceShape",
    "faceProportions",
    "furColors",
    "markings",
    "eyeShape",
    "eyeColor",
    "earShape",
    "bodyType",
    "tail",
    "signatureMarks",
    "temperament",
)
_TRAIT_KEY_SET = frozenset(TRAIT_KEYS)
_BODY_MODULES = frozenset(
    {"body-slender-v1", "body-balanced-v1", "body-rounded-v1"}
)
_PROFILE_FIELDS = frozenset(
    {
        "schemaVersion",
        "species",
        "style",
        "bodyModuleId",
        "bodyModuleSource",
        "traits",
        "completionSummary",
    }
)
_TRAIT_FIELDS = frozenset(
    {"key", "value", "source", "evidencePhotoIds"}
)
_SOURCES = frozenset({"user", "ai-completed"})
_MAX_TEXT = 512
_MAX_ID = 128
_MAX_ATLAS_BYTES = 20 * 1024 * 1024
_PNG_SIZE = (2048, 2048)
_GUIDE_ROOT = Path(__file__).resolve().parent / "assets" / "uv-guides"
_MODULE_ROOT = (
    Path(__file__).resolve().parents[4]
    / "apps"
    / "desktop"
    / "public"
    / "cat-character-modules"
    / "cat-a-live2d-v1"
)


@dataclass(frozen=True)
class TextureArtifact:
    png: bytes
    sha256: str
    provider_task_id: str
    body_module_id: str
    body_module_contract_sha256: str
    provider_raw_sha256: str = ""
    source_texture_sha256: str = ""
    source_alpha_sha256: str = ""
    work_canvas_sha256: str = ""
    region_map_sha256: str = ""
    composer_version: str = ""
    png_encoder_version: str = ""
    coverage_report: dict[str, object] = field(default_factory=dict)


@dataclass(frozen=True)
class SemanticGenerationInputs:
    revision: int
    identity_reference: SourceImage
    identity_reference_sha256: str
    profile_sha256: str


@dataclass(frozen=True)
class RenderedSemanticLayer:
    png: bytes
    provider_task_id: str


@dataclass(frozen=True)
class SemanticRenderRuntime:
    report_task_id: Callable[[str], None] | None
    poll_interval_seconds: float
    max_wait_seconds: float


def _identity_trait_schema() -> dict[str, Any]:
    return {
        "type": "object",
        "additionalProperties": False,
        "properties": {
            "key": {"type": "string", "enum": list(TRAIT_KEYS)},
            "value": {"type": "string", "minLength": 1, "maxLength": _MAX_TEXT},
            "source": {"type": "string", "enum": ["user"]},
            "evidencePhotoIds": {
                "type": "array",
                "items": {"type": "string", "minLength": 1, "maxLength": _MAX_ID},
                "minItems": 1,
            },
        },
        "required": ["key", "value", "source", "evidencePhotoIds"],
    }


def _profile_schema() -> dict[str, Any]:
    return {
        "type": "object",
        "additionalProperties": False,
        "properties": {
            "schemaVersion": {"type": "integer", "const": 1},
            "species": {"type": "string", "const": "cat"},
            "style": {"type": "string", "const": "animated-film-soft-v1"},
            "bodyModuleId": {"type": "string", "enum": sorted(_BODY_MODULES)},
            "bodyModuleSource": {
                "type": "string",
                "enum": ["user", "ai-completed"],
            },
            "traits": {
                "type": "array",
                "items": _identity_trait_schema(),
                "maxItems": len(TRAIT_KEYS),
            },
            "completionSummary": {"type": "array", "items": {"type": "string"}},
        },
        "required": sorted(_PROFILE_FIELDS),
    }


def _completion_schema() -> dict[str, Any]:
    return {
        "type": "object",
        "additionalProperties": False,
        "properties": {
            "requestedTraitKeys": {
                "type": "array",
                "items": {"type": "string", "enum": list(TRAIT_KEYS)},
            },
            "completedTraits": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": False,
                    "properties": {
                        "key": {"type": "string", "enum": list(TRAIT_KEYS)},
                        "value": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": _MAX_TEXT,
                        },
                        "source": {
                            "type": "string",
                            "enum": ["ai-completed"],
                        },
                        "evidencePhotoIds": {
                            "type": "array",
                            "items": {"type": "string"},
                            "maxItems": 0,
                        },
                    },
                    "required": ["key", "value", "source", "evidencePhotoIds"],
                },
                "maxItems": len(TRAIT_KEYS),
            },
            "bodyModuleId": {"type": "string", "enum": sorted(_BODY_MODULES)},
            "bodyModuleSource": {"type": "string", "enum": ["user", "ai-completed"]},
        },
        "required": [
            "requestedTraitKeys",
            "completedTraits",
            "bodyModuleId",
            "bodyModuleSource",
        ],
    }


def _require_mapping(value: object, label: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ContractError(f"{label} must be an object")
    return value


def _reject_unknown(value: Mapping[str, Any], allowed: frozenset[str], label: str) -> None:
    for field in value:
        if field not in allowed:
            raise ContractError(f"unknown {label} field: {field}")


def _string(value: object, label: str, *, max_length: int = _MAX_TEXT) -> str:
    if not isinstance(value, str):
        raise ContractError(f"{label} must be a string")
    normalized = value.strip()
    if not normalized:
        raise ContractError(f"{label} must be a non-empty string")
    if len(normalized) > max_length:
        raise ContractError(f"{label} is too long")
    return normalized


def _enum(value: object, label: str, choices: frozenset[str]) -> str:
    normalized = _string(value, label)
    if normalized not in choices:
        raise ContractError(f"{label} is not supported: {normalized}")
    return normalized


def _normalize_trait(value: object, index: int) -> dict[str, Any]:
    raw = _require_mapping(value, f"traits[{index}]")
    _reject_unknown(raw, _TRAIT_FIELDS, f"trait")
    missing = _TRAIT_FIELDS.difference(raw)
    if missing:
        raise ContractError(f"traits[{index}] missing field: {sorted(missing)[0]}")
    key = _string(raw["key"], f"traits[{index}].key")
    if key not in _TRAIT_KEY_SET:
        raise ContractError(f"unsupported trait key: {key}")
    source = _enum(raw["source"], f"traits[{index}].source", _SOURCES)
    evidence = raw["evidencePhotoIds"]
    if not isinstance(evidence, list):
        raise ContractError(f"traits[{index}].evidencePhotoIds must be an array")
    normalized_evidence = [
        _string(photo_id, f"traits[{index}].evidencePhotoIds[{photo_index}]", max_length=_MAX_ID)
        for photo_index, photo_id in enumerate(evidence)
    ]
    if len(set(normalized_evidence)) != len(normalized_evidence):
        raise ContractError(f"duplicate evidencePhotoIds for trait: {key}")
    if source == "user" and not normalized_evidence:
        raise ContractError(f"traits[{index}].evidencePhotoIds must contain at least one photo id")
    return {
        "key": key,
        "value": _string(raw["value"], f"traits[{index}].value"),
        "source": source,
        "evidencePhotoIds": sorted(normalized_evidence),
    }


def validate_profile(profile: object) -> dict[str, Any]:
    """Validate and return a deterministic, detached profile mapping."""

    raw = _require_mapping(profile, "profile")
    _reject_unknown(raw, _PROFILE_FIELDS, "profile")
    missing = _PROFILE_FIELDS.difference(raw)
    if missing:
        raise ContractError(f"missing profile field: {sorted(missing)[0]}")
    if isinstance(raw["schemaVersion"], bool) or raw["schemaVersion"] != 1:
        raise ContractError("schemaVersion must be 1")
    if raw["species"] != "cat":
        raise ContractError("species must be cat")
    if raw["style"] != "animated-film-soft-v1":
        raise ContractError("style must be animated-film-soft-v1")
    body_module_id = _enum(raw["bodyModuleId"], "bodyModuleId", _BODY_MODULES)
    body_module_source = _enum(raw["bodyModuleSource"], "bodyModuleSource", _SOURCES)

    raw_traits = raw["traits"]
    if not isinstance(raw_traits, list):
        raise ContractError("traits must be an array")
    if len(raw_traits) > len(TRAIT_KEYS):
        raise ContractError("traits contains too many entries")
    traits = [_normalize_trait(value, index) for index, value in enumerate(raw_traits)]
    keys = [value["key"] for value in traits]
    if len(set(keys)) != len(keys):
        duplicate = next(key for key in keys if keys.count(key) > 1)
        raise ContractError(f"duplicate trait key: {duplicate}")

    summary = raw["completionSummary"]
    if not isinstance(summary, list):
        raise ContractError("completionSummary must be an array")
    normalized_summary = sorted(
        {_string(value, f"completionSummary[{index}]") for index, value in enumerate(summary)}
    )
    for trait in traits:
        if trait["source"] == "ai-completed" and trait["key"] not in normalized_summary:
            raise ContractError(
                f"completionSummary must include ai-completed trait: {trait['key']}"
            )
    body_trait = next((value for value in traits if value["key"] == "bodyType"), None)
    if body_module_source == "user" and (
        body_trait is None
        or body_trait["source"] != "user"
        or not body_trait["evidencePhotoIds"]
    ):
        raise ContractError("user body module requires user bodyType evidence")
    return {
        "schemaVersion": 1,
        "species": "cat",
        "style": "animated-film-soft-v1",
        "bodyModuleId": body_module_id,
        "bodyModuleSource": body_module_source,
        "traits": sorted(traits, key=lambda value: TRAIT_KEYS.index(value["key"])),
        "completionSummary": normalized_summary,
    }


def lock_semantic_generation_inputs(
    request: StepRequest,
    identity_reference: SourceImage | None,
) -> SemanticGenerationInputs:
    if request.profile is None:
        raise ContractError("semantic layer generation requires profile")
    if identity_reference is None:
        raise ContractError("semantic layer generation requires identity reference")
    if identity_reference not in request.source_images:
        raise ContractError(
            "identity reference must be a complete source photo for this revision"
        )
    _validated_source_png(identity_reference)
    profile = validate_profile(request.profile)
    profile_json = json.dumps(profile, sort_keys=True, separators=(",", ":"))
    return SemanticGenerationInputs(
        revision=request.revision,
        identity_reference=identity_reference,
        identity_reference_sha256=identity_reference.sha256,
        profile_sha256=hashlib.sha256(profile_json.encode("utf-8")).hexdigest(),
    )


def validate_completion(
    before: object, after: object, locked_traits: Sequence[str]
) -> dict[str, Any]:
    """Validate a completed profile against its locked identity values."""

    before_profile = validate_profile(before)
    after_profile = validate_profile(after)
    if not isinstance(locked_traits, Sequence) or isinstance(locked_traits, (str, bytes)):
        raise ContractError("lockedTraits must be an array of strings")
    normalized_locked = [_string(value, "locked trait key") for value in locked_traits]
    if len(set(normalized_locked)) != len(normalized_locked):
        raise ContractError("lockedTraits contains duplicate trait key")
    for key in normalized_locked:
        if key not in _TRAIT_KEY_SET:
            raise ContractError(f"unsupported locked trait key: {key}")
        before_trait = next((value for value in before_profile["traits"] if value["key"] == key), None)
        after_trait = next((value for value in after_profile["traits"] if value["key"] == key), None)
        if before_trait != after_trait:
            raise ContractError(f"locked trait changed: {key}")
    if "bodyType" in normalized_locked and (
        before_profile["bodyModuleId"] != after_profile["bodyModuleId"]
        or before_profile["bodyModuleSource"] != after_profile["bodyModuleSource"]
    ):
        raise ContractError("locked body module changed")
    return after_profile


def _analysis_prompt() -> str:
    return (
        "Analyze only identity traits directly observed in the supplied pet photos. "
        "Do not infer, guess, complete, or substitute any missing trait. "
        "Use source=user and cite one or more evidencePhotoIds for every returned trait. "
        "If body shape is not observable, defer the body module with "
        "bodyModuleSource=ai-completed without adding a bodyType trait or evidence. "
        "Do not use a standard cat template; preserve only photo-grounded facts. "
        "Return the strict profile JSON and leave completionSummary empty."
    )


def _completion_prompt(profile: Mapping[str, Any], missing: Sequence[str]) -> str:
    return (
        "Complete only missing traits in this photo-avatar profile. "
        "Do not alter existing traits, species, style, or a user-selected body module. "
        "Return only missing traits with source=ai-completed and no evidencePhotoIds. "
        "Body module inference is allowed only when bodyModuleSource is ai-completed; "
        "never use a standard cat template. "
        f"only missing traits: {', '.join(missing)}. "
        "Current profile JSON: <PROFILE_JSON>"
        f"{json.dumps(profile, ensure_ascii=False, sort_keys=True, separators=(',', ':'))}"
        "</PROFILE_JSON>. Include bodyModuleId and bodyModuleSource."
    )


def analyze_identity(request: StepRequest, *, client: Any) -> dict[str, Any]:
    if request.step != "analyzeIdentity":
        raise ContractError("analyze_identity requires analyzeIdentity step")
    if request.profile is not None:
        raise ContractError("analyzeIdentity profile must be null")
    if request.locked_traits:
        raise ContractError("analyzeIdentity lockedTraits must be empty")
    response = client.analyze_json(
        _analysis_prompt(), [image.png for image in request.source_images], _profile_schema()
    )
    result = validate_profile(response)
    if any(value["source"] != "user" for value in result["traits"]):
        raise ContractError("analysis may only return user traits")
    if result["completionSummary"]:
        raise ContractError("analysis may not return completionSummary")
    return result


def complete_appearance(request: StepRequest, *, client: Any) -> dict[str, Any]:
    if request.step != "completeAppearance":
        raise ContractError("complete_appearance requires completeAppearance step")
    if request.profile is None:
        raise ContractError("completeAppearance requires profile")
    if request.source_images:
        raise ContractError("completeAppearance cannot carry source images")
    before = validate_profile(request.profile)
    present = {value["key"] for value in before["traits"]}
    missing = [key for key in TRAIT_KEYS if key not in present]
    response = client.analyze_json(
        _completion_prompt(before, missing), [], _completion_schema()
    )
    raw = _require_mapping(response, "completion response")
    expected_fields = frozenset(
        {"requestedTraitKeys", "completedTraits", "bodyModuleId", "bodyModuleSource"}
    )
    _reject_unknown(raw, expected_fields, "completion")
    if expected_fields.difference(raw):
        raise ContractError("completion response is missing a required field")
    requested = raw["requestedTraitKeys"]
    if not isinstance(requested, list) or any(not isinstance(value, str) for value in requested):
        raise ContractError("requestedTraitKeys must be an array of strings")
    if len(set(requested)) != len(requested):
        raise ContractError("duplicate requested trait key")
    if requested != missing:
        raise ContractError("requestedTraitKeys must contain exactly missing traits")
    completed_raw = raw["completedTraits"]
    if not isinstance(completed_raw, list):
        raise ContractError("completedTraits must be an array")
    completed = [_normalize_trait(value, index) for index, value in enumerate(completed_raw)]
    keys = [value["key"] for value in completed]
    if len(set(keys)) != len(keys):
        raise ContractError("duplicate completed trait key")
    if set(keys) != set(missing):
        raise ContractError("completedTraits must contain exactly missing traits")
    for value in completed:
        if value["source"] != "ai-completed":
            raise ContractError("completedTraits entries must use ai-completed source")
        if value["evidencePhotoIds"]:
            raise ContractError("completedTraits evidencePhotoIds must be empty")

    body_module_id = _enum(raw["bodyModuleId"], "bodyModuleId", _BODY_MODULES)
    body_module_source = _enum(raw["bodyModuleSource"], "bodyModuleSource", _SOURCES)
    if before["bodyModuleSource"] == "user" and (
        body_module_id != before["bodyModuleId"] or body_module_source != "user"
    ):
        raise ContractError("bodyModuleId cannot replace user-selected body module")
    combined = {value["key"]: value for value in before["traits"]}
    combined.update({value["key"]: value for value in completed})
    summary = set(before["completionSummary"])
    summary.update(missing)
    if body_module_source == "ai-completed":
        summary.add(f"体型: {body_module_id}")
    after = {
        "schemaVersion": 1,
        "species": "cat",
        "style": "animated-film-soft-v1",
        "bodyModuleId": body_module_id,
        "bodyModuleSource": body_module_source,
        "traits": [combined[key] for key in TRAIT_KEYS],
        "completionSummary": sorted(summary),
    }
    return validate_completion(before, after, request.locked_traits)


def render_texture_atlas(
    request: StepRequest,
    client: Any,
    guide_index: Mapping[str, Any],
    *,
    report_task_id: Callable[[str], None] | None = None,
    poll_interval_seconds: float = 5.0,
    max_wait_seconds: float = 300.0,
) -> TextureArtifact:
    if request.step != "renderTextureAtlas":
        raise ContractError("render_texture_atlas requires renderTextureAtlas step")
    if request.provider_session_id is None:
        raise ContractError("renderTextureAtlas requires providerSessionId")
    if request.profile is None:
        raise ContractError("renderTextureAtlas requires profile")
    profile = validate_profile(request.profile)
    profile_keys = {trait["key"] for trait in profile["traits"]}
    if profile_keys != _TRAIT_KEY_SET:
        raise ContractError("renderTextureAtlas requires a complete profile")
    if not 1 <= len(request.source_images) <= 8:
        raise ContractError("renderTextureAtlas requires all 1..8 source photos")

    module_id = profile["bodyModuleId"]
    entry = _guide_entry(guide_index, module_id)
    expected_contract_hash = entry["moduleContractSha256"]
    module_dir = _MODULE_ROOT / module_id
    contract_bytes = (module_dir / "模块.json").read_bytes()
    actual_contract_hash = hashlib.sha256(contract_bytes).hexdigest()
    if expected_contract_hash != actual_contract_hash:
        raise ContractError("UV guide contract hash does not match body module")
    if request.body_module_contract_sha256 != expected_contract_hash:
        raise ContractError("body module contract hash does not match UV guide")
    contract = json.loads(contract_bytes.decode("utf-8"))
    neutral_relative = contract.get("files", {}).get("neutralTexture")
    if not isinstance(neutral_relative, str):
        raise ContractError("body module neutral texture is invalid")
    try:
        neutral_path = resolve_module_file(module_dir, neutral_relative)
    except ValueError as exc:
        raise ContractError(str(exc)) from exc
    neutral_bytes = neutral_path.read_bytes()
    _, neutral_alpha = _decode_rgba_png(neutral_bytes, "body module neutral texture")
    source_texture_sha256 = hashlib.sha256(neutral_bytes).hexdigest()
    if source_texture_sha256 != entry.get("sourceTextureSha256"):
        raise ContractError("source texture hash does not match body module")
    if hashlib.sha256(neutral_alpha).hexdigest() != entry.get("sourceAlphaSha256"):
        raise ContractError("source alpha does not match body module")
    work_canvas = _read_indexed_asset(
        entry, module_id, "workCanvasPath", "workCanvasSha256", "work canvas"
    )
    region_map = _read_indexed_asset(
        entry, module_id, "regionMapPath", "regionMapSha256", "region map"
    )
    expected_bundle = build_work_canvas(neutral_bytes)
    if work_canvas != expected_bundle.work_canvas_png:
        raise ContractError("work canvas does not match deterministic body module canvas")
    if region_map != expected_bundle.region_map_png:
        raise ContractError("region map does not match deterministic body module regions")

    identity_inputs = lock_semantic_generation_inputs(request, request.source_images[0])
    module_snapshot = build_module_semantic_snapshot(
        module_id, contract_bytes, neutral_bytes
    )
    profile_json = json.dumps(profile, sort_keys=True, separators=(",", ":"))
    task_ids: list[str] = []

    def record_task_id(task_id: str) -> None:
        if not task_ids and report_task_id is not None:
            report_task_id(task_id)
        task_ids.append(task_id)

    runtime = SemanticRenderRuntime(
        report_task_id=record_task_id,
        poll_interval_seconds=poll_interval_seconds,
        max_wait_seconds=max_wait_seconds,
    )
    validated_layers: dict[str, ValidatedSemanticLayer] = {}
    layer_audits: list[SemanticLayerAuditV1] = []
    for spec in build_semantic_layer_specs(module_snapshot):
        for attempt in range(1, 4):
            try:
                rendered = render_one_semantic_layer(
                    client=client,
                    identity_reference=identity_inputs.identity_reference,
                    profile_json=profile_json,
                    spec=spec,
                    attempt=attempt,
                    runtime=runtime,
                )
                validated = validate_semantic_layer_png(rendered.png, spec)
            except (ContractError, Lk888Error) as exc:
                if isinstance(exc, Lk888Error) and not exc.retryable:
                    raise
                if attempt == 3:
                    raise
                continue
            validated_layers[spec.layer_id] = validated
            layer_audits.append(
                SemanticLayerAuditV1(
                    layer_id=spec.layer_id,
                    provider_raw_sha256=validated.provider_raw_sha256,
                    canonical_layer_sha256=validated.canonical_layer_sha256,
                    mask_sha256=validated.mask_sha256,
                    attempt=attempt,
                )
            )
            break
    if tuple(validated_layers) != SEMANTIC_LAYER_IDS:
        raise ContractError("semantic layer generation did not freeze every layer")
    canonical = compose_semantic_atlas(
        layers=tuple(validated_layers.values()), module_snapshot=module_snapshot
    )
    _reject_standard_cat_canonical(canonical.png)
    semantic_audit = SemanticAtlasAuditV1(
        identity_reference_sha256=identity_inputs.identity_reference_sha256,
        profile_sha256=identity_inputs.profile_sha256,
        layers=tuple(layer_audits),
        canonical_atlas_sha256=canonical.canonical_sha256,
        body_module_id=module_id,
    )
    return TextureArtifact(
        png=canonical.png,
        sha256=canonical.canonical_sha256,
        provider_task_id=task_ids[0],
        body_module_id=module_id,
        body_module_contract_sha256=expected_contract_hash,
        provider_raw_sha256=semantic_audit.immutable_digest(),
        source_texture_sha256=source_texture_sha256,
        source_alpha_sha256=canonical.source_alpha_sha256,
        work_canvas_sha256=hashlib.sha256(work_canvas).hexdigest(),
        region_map_sha256=hashlib.sha256(region_map).hexdigest(),
        composer_version=COMPOSER_VERSION,
        png_encoder_version=PNG_ENCODER_VERSION,
        coverage_report=semantic_audit.to_wire(),
    )


def render_one_semantic_layer(
    *,
    client: Any,
    identity_reference: SourceImage,
    profile_json: str,
    spec: SemanticLayerSpec,
    attempt: int,
    runtime: SemanticRenderRuntime,
) -> RenderedSemanticLayer:
    template = _semantic_layer_template(spec)
    prompt = (
        f"Render only semantic layer {spec.layer_id} as an exact "
        f"{spec.width}x{spec.height} RGBA PNG. Preserve the supplied mask boundary "
        "and transparent RGB. Use only the locked identity reference and profile. "
        f"Attempt {attempt}. Profile: {profile_json}"
    )
    task_id = client.submit_image(
        prompt, [identity_reference.png, template, spec.mask_png]
    )
    if not isinstance(task_id, str) or not task_id.strip():
        raise ContractError("provider image task id is invalid")
    if runtime.report_task_id is not None:
        runtime.report_task_id(task_id)
    deadline = time.monotonic() + runtime.max_wait_seconds
    while True:
        state = client.poll_image(task_id)
        if state.error is not None:
            raise state.error
        if state.is_final:
            break
        if time.monotonic() >= deadline:
            raise Lk888Error("timeout", True, "provider image task timed out")
        if runtime.poll_interval_seconds > 0:
            time.sleep(runtime.poll_interval_seconds)
    if state.state != "success" or not state.result_url:
        raise Lk888Error(
            "temporaryUnavailable", True, "provider image task did not succeed"
        )
    provider_png = client.download(state.result_url)
    if len(provider_png) > _MAX_ATLAS_BYTES:
        raise ContractError("provider texture exceeds 20 MiB")
    return RenderedSemanticLayer(png=provider_png, provider_task_id=task_id)


def _semantic_layer_template(spec: SemanticLayerSpec) -> bytes:
    mask = Image.open(BytesIO(spec.mask_png)).copy()
    template = Image.new("RGBA", (spec.width, spec.height), (0, 0, 0, 0))
    template.paste((127, 127, 127, 255), mask=mask)
    output = BytesIO()
    template.save(output, format="PNG", optimize=False, compress_level=9)
    return output.getvalue()


def _guide_entry(
    guide_index: Mapping[str, Any], module_id: str
) -> Mapping[str, Any]:
    if not isinstance(guide_index, Mapping):
        raise ContractError("UV guide index must be an object")
    if guide_index.get("schemaVersion") != 2:
        raise ContractError("UV guide index schema is invalid")
    if guide_index.get("generatorVersion") != GENERATOR_VERSION:
        raise ContractError("UV guide generator version is invalid")
    guides = guide_index.get("guides")
    if not isinstance(guides, list):
        raise ContractError("UV guide index entries are invalid")
    legacy_fields = {"relativePath", "guideSha256"}
    if any(
        isinstance(entry, Mapping) and legacy_fields.intersection(entry)
        for entry in guides
    ):
        raise ContractError("UV guide index contains a legacy field")
    entries = [
        entry
        for entry in guides
        if isinstance(entry, Mapping) and entry.get("moduleId") == module_id
    ]
    if len(entries) != 1:
        raise ContractError("matching UV guide is unavailable")
    return entries[0]


def _read_indexed_asset(
    entry: Mapping[str, Any],
    module_id: str,
    path_field: str,
    hash_field: str,
    label: str,
) -> bytes:
    suffix = "work.png" if path_field == "workCanvasPath" else "regions.png"
    relative = entry.get(path_field)
    if relative != f"{module_id}.{suffix}":
        raise ContractError(f"{label} path does not match body module")
    asset = (_GUIDE_ROOT / relative).read_bytes()
    if hashlib.sha256(asset).hexdigest() != entry.get(hash_field):
        raise ContractError(f"{label} hash does not match index")
    if entry.get("width") != 2048 or entry.get("height") != 2048:
        raise ContractError(f"{label} dimensions do not match index")
    return asset


def _validated_source_png(source: Any) -> bytes:
    if hashlib.sha256(source.png).hexdigest() != source.sha256:
        raise ContractError("source photo hash does not match bytes")
    try:
        with Image.open(BytesIO(source.png)) as image:
            image.load()
            if image.format != "PNG":
                raise ContractError("source photo must be a PNG")
            if image.size != (source.width, source.height):
                raise ContractError("source photo dimensions do not match")
    except (OSError, SyntaxError) as exc:
        raise ContractError("source photo must be a valid PNG") from exc
    return source.png


def _reject_standard_cat_canonical(canonical: bytes) -> None:
    image, _ = _decode_rgba_png(canonical, "canonical texture")
    canonical_pixels = _visible_rgba_sha256(image)
    for module_id in BODY_MODULE_IDS:
        contract = json.loads(
            (_MODULE_ROOT / module_id / "模块.json").read_text(encoding="utf-8")
        )
        try:
            neutral_path = resolve_module_file(
                _MODULE_ROOT / module_id, contract["files"]["neutralTexture"]
            )
        except ValueError as exc:
            raise ContractError(str(exc)) from exc
        neutral, _ = _decode_rgba_png(neutral_path.read_bytes(), "standard cat neutral")
        if _visible_rgba_sha256(neutral) == canonical_pixels:
            raise ContractError("canonical texture matches a standard cat neutral texture")


def _visible_rgba_sha256(image: Image.Image) -> str:
    pixels = bytearray(image.convert("RGBA").tobytes())
    for offset in range(0, len(pixels), 4):
        if pixels[offset + 3] == 0:
            pixels[offset : offset + 3] = b"\x00\x00\x00"
    return hashlib.sha256(pixels).hexdigest()


def _decode_rgba_png(png: bytes, label: str) -> tuple[Image.Image, bytes]:
    try:
        with Image.open(BytesIO(png)) as source:
            source.load()
            if source.format != "PNG":
                raise ContractError(f"{label} must be a PNG")
            if source.size != _PNG_SIZE:
                raise ContractError(f"{label} must be exactly 2048x2048")
            if source.mode != "RGBA":
                raise ContractError(
                    f"{label} must use native RGBA mode with an alpha channel"
                )
            image = source.convert("RGBA")
            return image, image.getchannel("A").tobytes()
    except (OSError, SyntaxError) as exc:
        raise ContractError(f"{label} must be a valid PNG") from exc

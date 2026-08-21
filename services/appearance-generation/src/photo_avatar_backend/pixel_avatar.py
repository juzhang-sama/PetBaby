from collections.abc import Callable, Sequence
from dataclasses import dataclass, replace
from datetime import datetime, timezone
import hashlib
import json
import time
from typing import Protocol

from .contracts import ContractError, PixelAppearanceProfile, PixelStepRequest
from .lk888_client import Lk888Error, MediaState
from .pixel_audit import (
    JsonValue,
    PixelAvatarAudit,
    PixelAvatarAuditV1,
    PixelAvatarAuditV2,
    parse_pixel_avatar_audit,
)
from .pixel_png import (
    PixelPngAudit,
    audit_pixel_png,
    normalize_pixel_png,
    postprocess_pixel_png,
)
from .pixel_prompt import (
    TRAIT_KEYS,
    analysis_prompt,
    completion_prompt,
    generation_prompt,
    profile_schema,
    profile_wire,
)
from .pixel_style import PixelStylePack


class PixelIdentityClient(Protocol):
    def analyze_json(
        self, prompt: str, images: Sequence[bytes], schema: dict[str, JsonValue]
    ) -> dict[str, JsonValue]: ...


class PixelImageClient(Protocol):
    def submit_image(self, prompt: str, images: Sequence[bytes]) -> str: ...

    def poll_image(self, task_id: str) -> MediaState: ...

    def download(self, url: str) -> bytes: ...


@dataclass(frozen=True, slots=True)
class PixelAvatarArtifact:
    png: bytes
    sha256: str
    width: int
    height: int
    audit: PixelAvatarAudit


def analyze_pixel_identity(
    request: PixelStepRequest, *, client: PixelIdentityClient
) -> dict[str, JsonValue]:
    if request.step != "analyzeIdentity" or request.profile is not None:
        raise ContractError("pixel identity analysis requires a null profile")
    images = tuple(image.png for image in request.source_images)
    observed = PixelAppearanceProfile.parse(
        client.analyze_json(
            analysis_prompt(request.style_profile_id),
            images,
            profile_schema(TRAIT_KEYS, "user", request.style_profile_id),
        )
    )
    if observed.style_profile_id != request.style_profile_id:
        raise ContractError("pixel analysis styleProfileId does not match request")
    if any(trait.source != "user" for trait in observed.traits) or observed.completion_summary:
        raise ContractError("pixel analysis may only return photo-grounded user traits")
    observed_by_key = {trait.key: trait for trait in observed.traits}
    if any(key not in observed_by_key for key in request.locked_traits):
        raise ContractError("locked pixel traits must be photo-grounded")
    missing = tuple(key for key in TRAIT_KEYS if key not in observed_by_key)
    completion = PixelAppearanceProfile.parse(
        client.analyze_json(
            completion_prompt(observed, request.modification, missing),
            images,
            profile_schema(missing, "ai-completed", request.style_profile_id),
        )
    )
    if completion.style_profile_id != request.style_profile_id:
        raise ContractError("pixel completion styleProfileId does not match request")
    completed_by_key = {trait.key: trait for trait in completion.traits}
    if (
        set(completed_by_key) != set(missing)
        or any(trait.source != "ai-completed" for trait in completion.traits)
        or tuple(completion.completion_summary) != missing
    ):
        raise ContractError("pixel completion must contain exactly the missing traits")
    merged = PixelAppearanceProfile(
        schema_version=1,
        species="cat",
        style_profile_id=request.style_profile_id,
        traits=tuple(
            observed_by_key.get(key) or completed_by_key[key] for key in TRAIT_KEYS
        ),
        completion_summary=missing,
    )
    return profile_wire(PixelAppearanceProfile.parse(profile_wire(merged)))


def generate_pixel_avatar(
    request: PixelStepRequest,
    *,
    client: PixelImageClient,
    style: PixelStylePack,
    report_task_id: Callable[[str], None] | None = None,
    poll_interval_seconds: float = 60.0,
    max_wait_seconds: float = 300.0,
) -> PixelAvatarArtifact:
    if request.step != "generatePixelAvatar" or request.profile is None:
        raise ContractError("pixel generation requires a complete profile")
    if request.provider_session_id is None:
        raise ContractError("pixel generation requires providerSessionId")
    if request.style_profile_id != style.style_profile_id:
        raise ContractError("pixel request styleProfileId does not match style pack")
    if request.profile.style_profile_id != request.style_profile_id:
        raise ContractError("pixel profile styleProfileId does not match request")
    identity_profile_wire = profile_wire(request.profile)
    raw_traits = identity_profile_wire["traits"]
    if not isinstance(raw_traits, list) or {
        trait["key"] for trait in raw_traits if isinstance(trait, dict)
    } != set(TRAIT_KEYS):
        raise ContractError("pixel generation requires all fifteen identity traits")
    identity_json = json.dumps(
        identity_profile_wire,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    images = [image.png for image in request.source_images]
    # 方案 A：不再把认可参考图作为图片参考传入。
    # 参考图作为图片会干扰 gpt-image-2 的风格锁定——模型倾向把它当「内容参考」，
    # 并忠实于用户照片（写实），导致产物偏离 pixel-style-v1 的硬边像素风。
    # 风格完全交由 _generation_prompt 的文本约束驱动。
    created_at = _timestamp()
    task_id = client.submit_image(generation_prompt(style, identity_json), images)
    if report_task_id is not None:
        report_task_id(task_id)
    deadline = time.monotonic() + max_wait_seconds
    while True:
        try:
            state = client.poll_image(task_id)
        except Lk888Error as error:
            if not error.retryable or time.monotonic() >= deadline:
                raise
            time.sleep(min(poll_interval_seconds, max(0.0, deadline - time.monotonic())))
            continue
        if state.error is not None:
            if not state.error.retryable or time.monotonic() >= deadline:
                raise state.error
            time.sleep(min(poll_interval_seconds, max(0.0, deadline - time.monotonic())))
            continue
        if state.state == "success" and state.result_url:
            break
        if time.monotonic() >= deadline:
            raise Lk888Error("timeout", True, "pixel image task timed out")
        time.sleep(min(poll_interval_seconds, max(0.0, deadline - time.monotonic())))
    if state.state != "success" or not state.result_url:
        raise Lk888Error("temporaryUnavailable", True, "pixel image task did not succeed")
    provider_png = client.download(state.result_url)
    normalized = normalize_pixel_png(
        provider_png,
        safe_margin_ratio=style.postprocess.safe_margin_ratio,
    )
    processed = postprocess_pixel_png(normalized, style.postprocess)
    checked = audit_pixel_png(processed.png)
    checked = replace(
        checked,
        provider_raw_sha256=hashlib.sha256(provider_png).hexdigest(),
    )
    identity_profile_sha256 = hashlib.sha256(identity_json.encode("utf-8")).hexdigest()
    completed_at = _timestamp()
    if processed.palette_report is None:
        audit: PixelAvatarAudit = PixelAvatarAuditV1(
            schema_version=1,
            session_id=request.session_id,
            revision=request.revision,
            attempt=request.attempt,
            provider="lk888",
            provider_model="gpt-image-2",
            provider_task_id=task_id,
            style_profile_id=style.style_profile_id,
            style_profile_sha256=style.profile_sha256,
            reference_sha256=style.reference_sha256,
            prompt_template_version=style.prompt_template_version,
            identity_profile_sha256=identity_profile_sha256,
            provider_raw_sha256=checked.provider_raw_sha256,
            normalized_sha256=checked.normalized_sha256,
            width=checked.width,
            height=checked.height,
            alpha_report=checked.alpha_report,
            privacy_policy_version="unverified",
            retention_policy="unverified",
            upstream_delete_api="unsupported",
            status="succeeded",
            error_code=None,
            created_at=created_at,
            completed_at=completed_at,
        )
    else:
        audit = PixelAvatarAuditV2(
            schema_version=2,
            session_id=request.session_id,
            revision=request.revision,
            attempt=request.attempt,
            provider="lk888",
            provider_model="gpt-image-2",
            provider_task_id=task_id,
            style_profile_id=style.style_profile_id,
            style_profile_sha256=style.profile_sha256,
            reference_sha256=style.reference_sha256,
            prompt_template_version=style.prompt_template_version,
            identity_profile_sha256=identity_profile_sha256,
            provider_raw_sha256=checked.provider_raw_sha256,
            normalized_sha256=checked.normalized_sha256,
            width=checked.width,
            height=checked.height,
            alpha_report=checked.alpha_report,
            privacy_policy_version="unverified",
            retention_policy="unverified",
            upstream_delete_api="unsupported",
            status="succeeded",
            error_code=None,
            created_at=created_at,
            completed_at=completed_at,
            logical_grid_size=style.postprocess.logical_grid_size,
            palette_color_limit=style.postprocess.palette_color_limit,
            visible_color_count=processed.palette_report.visible_color_count,
            quantize_method=style.postprocess.quantize_method,
            dither=style.postprocess.dither,
            protected_accent_slots=style.postprocess.protected_accent_slots,
            protected_accent_count=processed.palette_report.protected_accent_count,
            downsample=style.postprocess.downsample,
            upsample=style.postprocess.upsample,
        )
    audit = parse_pixel_avatar_audit(audit.to_wire())
    return PixelAvatarArtifact(
        png=checked.normalized_png,
        sha256=checked.normalized_sha256,
        width=checked.width,
        height=checked.height,
        audit=audit,
    )


def _timestamp() -> str:
    return datetime.now(timezone.utc).isoformat()


__all__ = [
    "PixelAvatarArtifact", "PixelPngAudit",
    "analyze_pixel_identity", "audit_pixel_png", "generate_pixel_avatar",
]

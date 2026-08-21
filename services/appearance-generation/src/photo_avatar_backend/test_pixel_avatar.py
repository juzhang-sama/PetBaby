from __future__ import annotations

import hashlib
from io import BytesIO

from PIL import Image, ImageDraw
import pytest

from .contracts import (
    ContractError,
    PixelAppearanceProfile,
    PixelStepRequest,
    SourceImage,
)
from .lk888_client import Lk888Error, MediaState
from .pixel_avatar import analyze_pixel_identity, audit_pixel_png, generate_pixel_avatar
from .pixel_png import normalize_pixel_png, pixelate_pixel_png
from .pixel_style import load_pixel_style_pack


TRAIT_KEYS = (
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
)


def _png(
    *,
    size: tuple[int, int] = (1024, 1024),
    boxes: tuple[tuple[int, int, int, int], ...] = ((64, 64, 960, 960),),
    alpha: int = 255,
    mode: str = "RGBA",
) -> bytes:
    image = Image.new(mode, size, (0, 0, 0, 0) if mode == "RGBA" else (0, 0, 0))
    draw = ImageDraw.Draw(image)
    fill = (120, 80, 40, alpha) if mode == "RGBA" else (120, 80, 40)
    for box in boxes:
        draw.rectangle(box, fill=fill)
    output = BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


def _profile_wire(
    *,
    observed_keys: tuple[str, ...],
    style_profile_id: str = "pixel-style-v1",
) -> dict[str, object]:
    observed = set(observed_keys)
    return {
        "schemaVersion": 1,
        "species": "cat",
        "styleProfileId": style_profile_id,
        "traits": [
            {
                "key": key,
                "value": f"value-{key}",
                "source": "user" if key in observed else "ai-completed",
                "evidencePhotoIds": ["front"] if key in observed else [],
            }
            for key in TRAIT_KEYS
        ],
        "completionSummary": [key for key in TRAIT_KEYS if key not in observed],
    }


def _request(
    *,
    step: str,
    profile: PixelAppearanceProfile | None,
    style_profile_id: str = "pixel-style-v1",
) -> PixelStepRequest:
    photo = _png()
    return PixelStepRequest(
        style_profile_id=style_profile_id,
        session_id="session-a",
        revision=2,
        provider_session_id=None if step == "analyzeIdentity" else "provider-a",
        step=step,
        attempt=1,
        consent_version="photo-avatar-third-party-ai-lk888-no-delete-v2",
        source_images=(
            SourceImage(
                source_id="front",
                png=photo,
                sha256=hashlib.sha256(photo).hexdigest(),
                width=1024,
                height=1024,
            ),
        ),
        profile=profile,
        modification=None,
        locked_traits=(),
    )


class IdentityClient:
    def __init__(self) -> None:
        self.calls: list[tuple[tuple[bytes, ...], dict[str, object]]] = []

    def analyze_json(
        self, prompt: str, images: tuple[bytes, ...] | list[bytes], schema: dict[str, object]
    ) -> dict[str, object]:
        self.calls.append((tuple(images), schema))
        if len(self.calls) == 1:
            return {
                "schemaVersion": 1,
                "species": "cat",
                "styleProfileId": "pixel-style-v1",
                "traits": [
                    {
                        "key": "faceShape",
                        "value": "round",
                        "source": "user",
                        "evidencePhotoIds": ["front"],
                    }
                ],
                "completionSummary": [],
            }
        return {
            "schemaVersion": 1,
            "species": "cat",
            "styleProfileId": "pixel-style-v1",
            "traits": [
                trait
                for trait in _profile_wire(observed_keys=())["traits"]
                if trait["key"] != "faceShape"
            ],
            "completionSummary": [key for key in TRAIT_KEYS if key != "faceShape"],
        }


class ImageClient:
    def __init__(self, png: bytes, *, poll_failures: int = 0) -> None:
        self.png = png
        self.prompts: list[str] = []
        self.images: tuple[bytes, ...] = ()
        self.poll_failures = poll_failures
        self.poll_calls = 0

    def submit_image(self, prompt: str, images: list[bytes]) -> str:
        self.prompts.append(prompt)
        self.images = tuple(images)
        return "108652999"

    def poll_image(self, task_id: str) -> MediaState:
        self.poll_calls += 1
        if self.poll_failures:
            self.poll_failures -= 1
            raise Lk888Error("network", True, "temporary network failure")
        return MediaState(task_id, "success", True, "https://example.invalid/pixel.png", None)

    def download(self, url: str) -> bytes:
        return self.png


class RetryableStateImageClient:
    def __init__(self, png: bytes) -> None:
        self.png = png
        self.submit_calls = 0
        self.poll_calls = 0

    def submit_image(self, prompt: str, images: list[bytes]) -> str:
        del prompt, images
        self.submit_calls += 1
        return "108652999"

    def poll_image(self, task_id: str) -> MediaState:
        self.poll_calls += 1
        if self.poll_calls == 1:
            return MediaState(
                task_id,
                "failed",
                True,
                None,
                Lk888Error("temporaryUnavailable", True, "provider still processing"),
            )
        return MediaState(task_id, "success", True, "https://example.invalid/pixel.png", None)

    def download(self, url: str) -> bytes:
        del url
        return self.png


class UnstableFinalStateImageClient(RetryableStateImageClient):
    def poll_image(self, task_id: str) -> MediaState:
        self.poll_calls += 1
        if self.poll_calls == 1:
            return MediaState(task_id, "cancelled", True, None, None)
        return MediaState(task_id, "success", True, "https://example.invalid/pixel.png", None)


def test_identity_analysis_calls_gpt4o_twice_and_merges_all_fifteen_traits() -> None:
    client = IdentityClient()

    result = analyze_pixel_identity(_request(step="analyzeIdentity", profile=None), client=client)

    assert len(client.calls) == 2
    assert {trait["key"] for trait in result["traits"]} == set(TRAIT_KEYS)
    assert result["completionSummary"] == [key for key in TRAIT_KEYS if key != "faceShape"]
    assert all(
        trait["evidencePhotoIds"] if trait["source"] == "user" else not trait["evidencePhotoIds"]
        for trait in result["traits"]
    )


def test_generation_emits_bound_audit_without_style_reference() -> None:
    style = load_pixel_style_pack("pixel-style-v1")
    client = ImageClient(_png())
    profile = PixelAppearanceProfile.parse(_profile_wire(observed_keys=("faceShape",)))
    request = _request(step="generatePixelAvatar", profile=profile)
    task_ids: list[str] = []

    artifact = generate_pixel_avatar(
        request,
        client=client,
        style=style,
        report_task_id=task_ids.append,
        max_wait_seconds=0,
    )

    # 方案 A：不再传入认可参考图，只传用户照片
    assert len(client.images) == 1
    assert client.images[0] == request.source_images[0].png
    assert task_ids == ["108652999"]
    assert artifact.audit.provider_model == "gpt-image-2"
    assert artifact.audit.style_profile_id == "pixel-style-v1"
    assert artifact.audit.reference_sha256 == style.reference_sha256
    assert artifact.audit.provider_task_id == "108652999"
    assert artifact.audit.normalized_sha256 == hashlib.sha256(artifact.png).hexdigest()


def test_v2_generation_emits_animation_ready_audit() -> None:
    style = load_pixel_style_pack("pixel-style-v2-animation-ready")
    client = ImageClient(_png())
    profile = PixelAppearanceProfile.parse(
        _profile_wire(
            observed_keys=("faceShape",),
            style_profile_id="pixel-style-v2-animation-ready",
        )
    )

    artifact = generate_pixel_avatar(
        _request(
            step="generatePixelAvatar",
            profile=profile,
            style_profile_id="pixel-style-v2-animation-ready",
        ),
        client=client,
        style=style,
        max_wait_seconds=0,
    )

    assert artifact.audit.schema_version == 2
    assert artifact.audit.style_profile_id == "pixel-style-v2-animation-ready"
    assert artifact.audit.width == 1024
    assert artifact.audit.height == 1024
    assert artifact.audit.to_wire()["logicalGridSize"] == 160
    assert artifact.audit.to_wire()["paletteColorLimit"] == 24


def test_generation_prompt_requires_transparent_edge_margin() -> None:
    style = load_pixel_style_pack("pixel-style-v1")
    client = ImageClient(_png())
    profile = PixelAppearanceProfile.parse(_profile_wire(observed_keys=("faceShape",)))

    generate_pixel_avatar(
        _request(step="generatePixelAvatar", profile=profile),
        client=client,
        style=style,
        max_wait_seconds=0,
    )

    assert any("at least 4% transparent margin" in prompt for prompt in client.prompts)


def test_generation_recovers_poll_network_error_without_resubmitting_task() -> None:
    style = load_pixel_style_pack("pixel-style-v1")
    client = ImageClient(_png(), poll_failures=1)
    profile = PixelAppearanceProfile.parse(_profile_wire(observed_keys=("faceShape",)))
    task_ids: list[str] = []

    artifact = generate_pixel_avatar(
        _request(step="generatePixelAvatar", profile=profile),
        client=client,
        style=style,
        report_task_id=task_ids.append,
        poll_interval_seconds=0.01,
        max_wait_seconds=1,
    )

    assert artifact.audit.provider_task_id == "108652999"
    assert task_ids == ["108652999"]
    assert client.poll_calls == 2


def test_generation_retries_retryable_state_error_without_resubmitting_task() -> None:
    style = load_pixel_style_pack("pixel-style-v1")
    client = RetryableStateImageClient(_png())
    profile = PixelAppearanceProfile.parse(_profile_wire(observed_keys=("faceShape",)))

    artifact = generate_pixel_avatar(
        _request(step="generatePixelAvatar", profile=profile),
        client=client,
        style=style,
        poll_interval_seconds=0,
        max_wait_seconds=1,
    )

    assert artifact.audit.provider_task_id == "108652999"
    assert client.submit_calls == 1
    assert client.poll_calls == 2


def test_generation_retries_unstable_final_state_without_resubmitting_task() -> None:
    style = load_pixel_style_pack("pixel-style-v1")
    client = UnstableFinalStateImageClient(_png())
    profile = PixelAppearanceProfile.parse(_profile_wire(observed_keys=("faceShape",)))

    artifact = generate_pixel_avatar(
        _request(step="generatePixelAvatar", profile=profile),
        client=client,
        style=style,
        poll_interval_seconds=0,
        max_wait_seconds=1,
    )

    assert artifact.audit.provider_task_id == "108652999"
    assert client.submit_calls == 1
    assert client.poll_calls == 2


def test_png_mechanical_gate_accepts_one_complete_hard_edge_subject() -> None:
    result = audit_pixel_png(_png())

    assert result.width == 1024
    assert result.height == 1024
    assert result.largest_component_share == 1.0
    assert result.partial_alpha_ratio == 0.0


def test_png_normalization_centers_a_soft_edge_touching_subject_for_strict_audit() -> None:
    source = _png(
        size=(2048, 2048),
        boxes=((0, 5, 2019, 2047),),
        alpha=128,
    )
    with pytest.raises(ContractError, match="margin"):
        audit_pixel_png(source)

    normalized = normalize_pixel_png(source)
    result = audit_pixel_png(normalized)

    assert (result.width, result.height) == (2048, 2048)
    assert result.partial_alpha_ratio == 0.0
    with Image.open(BytesIO(normalized)) as image:
        left, top, right, bottom = image.getchannel("A").getbbox()
        assert min(left, top, 2048 - right, 2048 - bottom) >= 82
        assert image.getpixel((1024, 1024)) == (120, 80, 40, 255)


@pytest.mark.parametrize(
    ("png", "message"),
    [
        (_png(size=(1023, 1024), boxes=((64, 64, 959, 960),)), "dimensions"),
        (_png(boxes=((0, 64, 960, 960),)), "margin"),
        (_png(boxes=((64, 64, 430, 960), (594, 64, 960, 960))), "connected"),
        (_png(alpha=128), "partial alpha"),
        (_png(mode="RGB"), "margin"),
    ],
    ids=("dimensions", "margin", "connected", "partial-alpha", "rgb"),
)
def test_png_mechanical_gate_rejects_unusable_provider_output(
    png: bytes, message: str
) -> None:
    with pytest.raises(ContractError, match=message):
        audit_pixel_png(png)


def _normalized_subject() -> bytes:
    return normalize_pixel_png(
        _png(size=(2048, 2048), boxes=((100, 100, 1900, 1900),))
    )


def test_pixelate_reduces_to_limited_palette_and_preserves_size() -> None:
    result = pixelate_pixel_png(_normalized_subject())

    with Image.open(BytesIO(result)) as image:
        assert image.size == (2048, 2048)
        assert image.mode == "RGBA"
        rgb = image.convert("RGB")
        alpha = image.getchannel("A")
        colors = {
            rgb.getpixel((x, y))
            for y in range(0, 2048, 8)
            for x in range(0, 2048, 8)
            if alpha.getpixel((x, y)) > 0
        }
        assert len(colors) <= 32


def test_pixelate_preserves_alpha_and_hard_transparency() -> None:
    source = _normalized_subject()
    before = audit_pixel_png(source)
    after = audit_pixel_png(pixelate_pixel_png(source))

    assert after.partial_alpha_ratio == 0.0
    assert after.largest_component_share == pytest.approx(
        before.largest_component_share, abs=1e-4
    )


@pytest.mark.parametrize(
    ("target_size", "palette_colors"),
    [(1, 32), (1025, 32), (512, 1), (512, 257)],
    ids=("target-too-small", "target-too-large", "palette-too-small", "palette-too-large"),
)
def test_pixelate_rejects_out_of_range_parameters(
    target_size: int, palette_colors: int
) -> None:
    with pytest.raises(ContractError):
        pixelate_pixel_png(_normalized_subject(), target_size=target_size, palette_colors=palette_colors)

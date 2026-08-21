from __future__ import annotations

import base64
from dataclasses import replace
import hashlib
from io import BytesIO
import json
from pathlib import Path
import shutil

import pytest
from PIL import Image

from . import pipelines
from .contracts import ContractError, SourceImage, StepRequest
from .lk888_client import Lk888Error, MediaState
from .semantic_layers import SEMANTIC_LAYER_IDS
from .pipelines import (
    TRAIT_KEYS,
    TextureArtifact,
    lock_semantic_generation_inputs,
    render_texture_atlas,
    validate_profile,
)


REPO_ROOT = Path(__file__).resolve().parents[4]
GUIDE_ROOT = Path(__file__).resolve().parent / "assets" / "uv-guides"
GUIDE_INDEX = json.loads((GUIDE_ROOT / "索引.json").read_text(encoding="utf-8"))
MODULE_ROOT = (
    REPO_ROOT
    / "apps"
    / "desktop"
    / "public"
    / "cat-character-modules"
    / "cat-a-live2d-v1"
)


def _png(
    color: tuple[int, ...],
    *,
    size: tuple[int, int] = (2048, 2048),
    mode: str = "RGBA",
) -> bytes:
    buffer = BytesIO()
    Image.new(mode, size, color).save(buffer, format="PNG")
    return buffer.getvalue()


def _source(source_id: str, color: tuple[int, int, int, int]) -> SourceImage:
    png = _png(color, size=(256, 256))
    return SourceImage(
        source_id=source_id,
        png=png,
        sha256=hashlib.sha256(png).hexdigest(),
        width=256,
        height=256,
    )


def _profile(module_id: str = "body-balanced-v1") -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "species": "cat",
        "style": "animated-film-soft-v1",
        "bodyModuleId": module_id,
        "bodyModuleSource": "ai-completed",
        "traits": [
            {
                "key": key,
                "value": f"photo avatar {key}",
                "source": "ai-completed",
                "evidencePhotoIds": [],
            }
            for key in TRAIT_KEYS
        ],
        "completionSummary": list(TRAIT_KEYS) + [f"体型: {module_id}"],
    }


def _guide_entry(module_id: str = "body-balanced-v1") -> dict[str, object]:
    return next(
        entry for entry in GUIDE_INDEX["guides"] if entry["moduleId"] == module_id
    )


def _request(module_id: str = "body-balanced-v1") -> StepRequest:
    return StepRequest(
        session_id="session-1",
        revision=2,
        provider_session_id="provider-1",
        step="renderTextureAtlas",
        attempt=1,
        consent_version="photo-avatar-third-party-ai-lk888-no-delete-v2",
        source_images=(
            _source("front", (110, 80, 60, 255)),
            _source("side", (80, 60, 45, 255)),
            _source("back", (60, 45, 35, 255)),
        ),
        profile=_profile(module_id),
        body_module_contract_sha256=str(
            _guide_entry(module_id)["moduleContractSha256"]
        ),
        modification=None,
        locked_traits=(),
    )


def _wire_request(module_id: str = "body-balanced-v1") -> dict[str, object]:
    request = _request(module_id)
    return {
        "sessionId": request.session_id,
        "revision": request.revision,
        "providerSessionId": request.provider_session_id,
        "step": request.step,
        "attempt": request.attempt,
        "consentVersion": request.consent_version,
        "sourceImages": [
            {
                "sourceId": source.source_id,
                "pngBase64": base64.b64encode(source.png).decode("ascii"),
                "sha256": source.sha256,
                "width": source.width,
                "height": source.height,
            }
            for source in request.source_images
        ],
        "profile": request.profile,
        "bodyModuleContractSha256": request.body_module_contract_sha256,
        "modification": request.modification,
        "lockedTraits": list(request.locked_traits),
    }


class FakeImageClient:
    def __init__(self, output: bytes):
        self.output = output
        self.prompt = ""
        self.images: list[bytes] = []
        self.submissions: list[tuple[str, list[bytes]]] = []
        self.task_prompts: dict[str, str] = {}

    def submit_image(self, prompt: str, images: list[bytes]) -> str:
        self.prompt = prompt
        self.images = list(images)
        self.submissions.append((prompt, list(images)))
        task_id = f"lk-task-{len(self.submissions)}"
        self.task_prompts[task_id] = prompt
        return task_id

    def poll_image(self, task_id: str) -> MediaState:
        assert task_id in self.task_prompts
        return MediaState(
            task_id,
            "success",
            True,
            f"https://fake.invalid/{task_id}.png",
            None,
        )

    def download(self, url: str) -> bytes:
        assert url.startswith("https://fake.invalid/lk-task-")
        return self._semantic_output()

    def _semantic_output(self) -> bytes:
        try:
            with Image.open(BytesIO(self.output)) as source:
                source.load()
                opaque = source.mode == "RGB" or (
                    source.mode == "RGBA" and source.getchannel("A").getextrema() == (255, 255)
                )
                if source.size != (2048, 2048) or not opaque:
                    return self.output
                with Image.open(BytesIO(self.images[2])) as mask_source:
                    mask = mask_source.convert("L")
                layer = source.convert("RGBA")
                layer.putalpha(mask)
                transparent = Image.new("RGBA", layer.size, (0, 0, 0, 0))
                transparent.paste(layer, mask=mask)
                output = BytesIO()
                transparent.save(output, format="PNG")
                return output.getvalue()
        except (OSError, SyntaxError):
            return self.output


class SequencedImageClient(FakeImageClient):
    def __init__(self, output: bytes):
        super().__init__(output)
        self.submit_calls = 0
        self.poll_calls = 0

    def submit_image(self, prompt: str, images: list[bytes]) -> str:
        self.submit_calls += 1
        return super().submit_image(prompt, images)

    def poll_image(self, task_id: str) -> MediaState:
        self.poll_calls += 1
        if self.poll_calls == 1:
            return MediaState(task_id, "queued", False, None, None)
        if self.poll_calls == 2:
            return MediaState(task_id, "running", False, None, None)
        return super().poll_image(task_id)


class FailingLayerClient(FakeImageClient):
    def __init__(
        self,
        output: bytes,
        *,
        layer_id: str,
        failing_attempts: frozenset[int],
        retryable: bool,
    ):
        super().__init__(output)
        self.layer_id = layer_id
        self.failing_attempts = failing_attempts
        self.retryable = retryable

    def poll_image(self, task_id: str) -> MediaState:
        prompt = self.task_prompts[task_id]
        attempt = next(
            attempt for attempt in range(1, 4) if f"Attempt {attempt}." in prompt
        )
        if f"semantic layer {self.layer_id}" in prompt and attempt in self.failing_attempts:
            error = Lk888Error("temporaryUnavailable", self.retryable, "planned layer failure")
            return MediaState(task_id, "failed", True, None, error)
        return super().poll_image(task_id)


def _valid_provider(*, mode: str = "RGB") -> bytes:
    return _png((42, 91, 137) if mode == "RGB" else (42, 91, 137, 255), mode=mode)


def _neutral_png(module_id: str = "body-balanced-v1") -> bytes:
    module = json.loads(
        (MODULE_ROOT / module_id / "模块.json").read_text(encoding="utf-8")
    )
    return (MODULE_ROOT / module_id / module["files"]["neutralTexture"]).read_bytes()


def _alpha(png: bytes) -> bytes:
    with Image.open(BytesIO(png)) as image:
        return image.convert("RGBA").getchannel("A").tobytes()


def _v2_index() -> dict[str, object]:
    index = json.loads(json.dumps(GUIDE_INDEX))
    index["schemaVersion"] = 2
    return index


def test_semantic_generation_locks_reference_and_profile_hash_to_request_revision():
    request = _request()
    inputs = lock_semantic_generation_inputs(request, request.source_images[0])

    assert inputs.revision == request.revision
    assert inputs.identity_reference == request.source_images[0]
    assert inputs.identity_reference_sha256 == request.source_images[0].sha256
    assert inputs.profile_sha256 == hashlib.sha256(
        json.dumps(
            validate_profile(request.profile), sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
    ).hexdigest()


def test_semantic_generation_rejects_missing_reference_before_provider_submission():
    client = FakeImageClient(_valid_provider())

    with pytest.raises(ContractError, match="identity reference"):
        lock_semantic_generation_inputs(_request(), None)

    assert client.images == []


def test_semantic_generation_rejects_reference_not_from_request_revision():
    with pytest.raises(ContractError, match="complete source photo"):
        lock_semantic_generation_inputs(
            _request(), _source("other", (12, 34, 56, 255))
        )


@pytest.mark.parametrize("mode", ("RGB", "RGBA"))
def test_render_texture_atlas_composes_opaque_provider_into_canonical_texture(
    mode: str,
):
    request = _request()
    client = FakeImageClient(_valid_provider(mode=mode))

    artifact = render_texture_atlas(request, client, _v2_index())

    assert len(client.submissions) == len(SEMANTIC_LAYER_IDS)
    assert all(images[0] == request.source_images[0].png for _, images in client.submissions)
    assert all(len(images) == 3 for _, images in client.submissions)
    assert artifact.provider_raw_sha256 != hashlib.sha256(client.output).hexdigest()
    assert artifact.sha256 == hashlib.sha256(artifact.png).hexdigest()
    assert artifact.png != client.output
    assert _alpha(artifact.png) == _alpha(_neutral_png())
    with Image.open(BytesIO(artifact.png)) as canonical:
        assert all(
            pixel[:3] == (0, 0, 0)
            for pixel in canonical.convert("RGBA").get_flattened_data()
            if pixel[3] == 0
        )


def test_render_texture_atlas_rejects_schema_v1_index():
    index = json.loads(json.dumps(GUIDE_INDEX))
    index["schemaVersion"] = 1

    with pytest.raises(ContractError, match="schema"):
        render_texture_atlas(_request(), FakeImageClient(_valid_provider()), index)


@pytest.mark.parametrize("legacy_field", ("relativePath", "guideSha256"))
def test_render_texture_atlas_rejects_legacy_fields_in_schema_v2_index(
    legacy_field: str,
):
    index = _v2_index()
    guides = index["guides"]
    assert isinstance(guides, list)
    guides[0][legacy_field] = "legacy"

    with pytest.raises(ContractError, match="legacy"):
        render_texture_atlas(_request(), FakeImageClient(_valid_provider()), index)


@pytest.mark.parametrize(
    ("path_field", "hash_field", "message"),
    (
        ("workCanvasPath", "workCanvasSha256", "work canvas hash"),
        ("regionMapPath", "regionMapSha256", "region map hash"),
    ),
)
def test_render_texture_atlas_rejects_indexed_asset_hash_tampering(
    path_field: str, hash_field: str, message: str
):
    index = _v2_index()
    entry = _guide_entry_from(index)
    assert entry[path_field]
    entry[hash_field] = "0" * 64

    with pytest.raises(ContractError, match=message):
        render_texture_atlas(_request(), FakeImageClient(_valid_provider()), index)


def test_render_texture_atlas_no_longer_uses_legacy_whole_atlas_change_ratio():
    work_path = GUIDE_ROOT / str(_guide_entry()["workCanvasPath"])
    with Image.open(work_path) as work:
        buffer = BytesIO()
        work.convert("RGB").save(buffer, format="PNG")

    artifact = render_texture_atlas(
        _request(), FakeImageClient(buffer.getvalue()), _v2_index()
    )

    assert artifact.coverage_report["layers"]


def test_render_texture_atlas_rejects_transparent_provider_rgba():
    with pytest.raises(ContractError, match="mask"):
        render_texture_atlas(
            _request(), FakeImageClient(_png((42, 91, 137, 254))), _v2_index()
        )


def test_render_texture_atlas_rejects_standard_cat_canonical():
    with Image.open(BytesIO(_neutral_png())) as neutral:
        buffer = BytesIO()
        neutral.convert("RGB").save(buffer, format="PNG")

    with pytest.raises(ContractError, match="standard cat"):
        render_texture_atlas(
            _request(), FakeImageClient(buffer.getvalue()), _v2_index()
        )


def test_render_texture_atlas_locks_one_identity_reference_for_all_semantic_layers():
    request = _request()
    client = FakeImageClient(_valid_provider())

    artifact = render_texture_atlas(request, client, GUIDE_INDEX)

    profile_json = json.dumps(
        validate_profile(request.profile), sort_keys=True, separators=(",", ":")
    )
    assert len(client.submissions) == len(SEMANTIC_LAYER_IDS)
    assert all(images[0] == request.source_images[0].png for _, images in client.submissions)
    assert all(profile_json in prompt for prompt, _ in client.submissions)
    assert [
        layer_id
        for layer_id in SEMANTIC_LAYER_IDS
        if any(f"semantic layer {layer_id}" in prompt for prompt, _ in client.submissions)
    ] == list(SEMANTIC_LAYER_IDS)
    assert isinstance(artifact, TextureArtifact)
    assert artifact.png != client.output
    assert artifact.coverage_report["bodyModuleId"] == "body-balanced-v1"
    assert artifact.sha256 == hashlib.sha256(artifact.png).hexdigest()
    assert artifact.provider_task_id == "lk-task-1"


def test_render_texture_atlas_submits_each_layer_and_polls_until_final():
    client = SequencedImageClient(_valid_provider())
    reported: list[str] = []

    artifact = render_texture_atlas(
        _request(),
        client,
        GUIDE_INDEX,
        report_task_id=reported.append,
        poll_interval_seconds=0,
    )

    assert artifact.provider_task_id == "lk-task-1"
    assert client.submit_calls == len(SEMANTIC_LAYER_IDS)
    assert client.poll_calls == len(SEMANTIC_LAYER_IDS) + 2
    assert reported == ["lk-task-1"]


def test_render_texture_atlas_retries_only_the_failed_face_layer():
    client = FailingLayerClient(
        _valid_provider(),
        layer_id="face",
        failing_attempts=frozenset({1}),
        retryable=True,
    )

    artifact = render_texture_atlas(_request(), client, GUIDE_INDEX)

    prompts = [prompt for prompt, _ in client.submissions]
    assert sum("semantic layer face" in prompt for prompt in prompts) == 2
    assert all(
        sum(f"semantic layer {layer_id}" in prompt for prompt in prompts) == 1
        for layer_id in SEMANTIC_LAYER_IDS
        if layer_id != "face"
    )
    face_audit = next(
        layer for layer in artifact.coverage_report["layers"] if layer["layerId"] == "face"
    )
    assert face_audit["attempt"] == 2


def test_render_texture_atlas_does_not_retry_non_retryable_layer_error():
    client = FailingLayerClient(
        _valid_provider(),
        layer_id="body-base",
        failing_attempts=frozenset({1}),
        retryable=False,
    )

    with pytest.raises(Lk888Error, match="planned layer failure"):
        render_texture_atlas(_request(), client, GUIDE_INDEX)

    assert len(client.submissions) == 1


def test_render_texture_atlas_does_not_compose_after_three_layer_failures(
    monkeypatch: pytest.MonkeyPatch,
):
    client = FailingLayerClient(
        _valid_provider(),
        layer_id="face",
        failing_attempts=frozenset({1, 2, 3}),
        retryable=True,
    )
    composed = False

    def record_unexpected_composition(**_kwargs: object) -> None:
        nonlocal composed
        composed = True

    monkeypatch.setattr(pipelines, "compose_semantic_atlas", record_unexpected_composition)

    with pytest.raises(Lk888Error, match="planned layer failure"):
        render_texture_atlas(_request(), client, GUIDE_INDEX)

    assert composed is False
    prompts = [prompt for prompt, _ in client.submissions]
    assert sum("semantic layer face" in prompt for prompt in prompts) == 3


def test_wire_request_parses_and_reaches_semantic_pipeline_with_locked_reference():
    request = StepRequest.parse(_wire_request())
    client = FakeImageClient(_valid_provider())

    render_texture_atlas(request, client, GUIDE_INDEX)

    assert len(client.submissions) == len(SEMANTIC_LAYER_IDS)
    assert all(images[0] == request.source_images[0].png for _, images in client.submissions)


def test_render_texture_atlas_rejects_wrong_contract_hash():
    request = replace(_request(), body_module_contract_sha256="0" * 64)

    with pytest.raises(ContractError, match="contract hash"):
        render_texture_atlas(request, FakeImageClient(_valid_provider()), GUIDE_INDEX)


def test_render_texture_atlas_rejects_index_and_request_with_same_wrong_contract_hash():
    index = json.loads(json.dumps(GUIDE_INDEX))
    _guide_entry_from(index)["moduleContractSha256"] = "0" * 64
    request = replace(_request(), body_module_contract_sha256="0" * 64)

    with pytest.raises(ContractError, match="contract hash"):
        render_texture_atlas(request, FakeImageClient(_valid_provider()), index)


def test_render_texture_atlas_rejects_synchronously_tampered_work_canvas_and_index(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    index = json.loads(json.dumps(GUIDE_INDEX))
    entry = _guide_entry_from(index)
    image = Image.new("RGBA", (2048, 2048), (17, 19, 23, 255))
    image.paste((47, 138, 151, 255), (100, 100, 900, 900))
    buffer = BytesIO()
    image.save(buffer, format="PNG")
    tampered = buffer.getvalue()
    (tmp_path / "body-balanced-v1.work.png").write_bytes(tampered)
    (tmp_path / "body-balanced-v1.regions.png").write_bytes(
        (GUIDE_ROOT / "body-balanced-v1.regions.png").read_bytes()
    )
    entry["workCanvasSha256"] = hashlib.sha256(tampered).hexdigest()
    monkeypatch.setattr(pipelines, "_GUIDE_ROOT", tmp_path)

    with pytest.raises(ContractError, match="deterministic"):
        render_texture_atlas(_request(), FakeImageClient(_valid_provider()), index)


def test_render_texture_atlas_rejects_cross_module_contract_mix():
    index = json.loads(json.dumps(GUIDE_INDEX))
    balanced = _guide_entry_from(index)
    slender = next(
        entry for entry in index["guides"] if entry["moduleId"] == "body-slender-v1"
    )
    balanced["moduleContractSha256"] = slender["moduleContractSha256"]
    request = replace(
        _request(),
        body_module_contract_sha256=str(slender["moduleContractSha256"]),
    )

    with pytest.raises(ContractError, match="contract hash"):
        render_texture_atlas(request, FakeImageClient(_valid_provider()), index)


@pytest.mark.parametrize(
    ("output", "message"),
    [
        (_png((10, 20, 30), size=(1024, 2048), mode="RGB"), "dimensions"),
        (_png((10, 128), mode="LA"), "RGB"),
        (_png((10, 20, 30, 254)), "mask"),
        (b"x" * (20 * 1024 * 1024 + 1), "20 MiB"),
    ],
    ids=("wrong-size", "unsupported-mode", "transparent-rgba", "too-large"),
)
def test_render_texture_atlas_rejects_invalid_provider_artifact(
    output: bytes, message: str
):
    with pytest.raises(ContractError, match=message):
        render_texture_atlas(_request(), FakeImageClient(output), GUIDE_INDEX)


def test_render_texture_atlas_rejects_palette_provider_artifact():
    palette = Image.new("P", (2048, 2048), 0)
    palette.putpalette([10, 20, 30] + [0, 0, 0] * 255)
    palette.info["transparency"] = 0
    palette_buffer = BytesIO()
    palette.save(palette_buffer, format="PNG")

    with pytest.raises(ContractError, match="RGB"):
        render_texture_atlas(
            _request(), FakeImageClient(palette_buffer.getvalue()), GUIDE_INDEX
        )


@pytest.mark.parametrize("module_id", ("body-slender-v1", "body-balanced-v1", "body-rounded-v1"))
def test_render_texture_atlas_rejects_every_standard_cat_neutral_texture(module_id: str):
    module = json.loads(
        (MODULE_ROOT / module_id / "模块.json").read_text(encoding="utf-8")
    )
    neutral = (MODULE_ROOT / module_id / module["files"]["neutralTexture"]).read_bytes()

    with Image.open(BytesIO(neutral)) as source:
        buffer = BytesIO()
        source.convert("RGB").save(buffer, format="PNG")

    with pytest.raises(ContractError, match="standard cat"):
        render_texture_atlas(
            _request(module_id), FakeImageClient(buffer.getvalue()), GUIDE_INDEX
        )


def _guide_entry_from(index: dict[str, object]) -> dict[str, object]:
    guides = index["guides"]
    assert isinstance(guides, list)
    return next(entry for entry in guides if entry["moduleId"] == "body-balanced-v1")


@pytest.mark.parametrize("path_kind", ("traversal", "absolute"))
def test_render_texture_atlas_rejects_neutral_texture_outside_module_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, path_kind: str
):
    module_root = tmp_path / "cat-a-live2d-v1"
    for module_id in ("body-slender-v1", "body-balanced-v1", "body-rounded-v1"):
        source_dir = MODULE_ROOT / module_id
        target_dir = module_root / module_id
        target_dir.mkdir(parents=True)
        shutil.copy2(source_dir / "模块.json", target_dir / "模块.json")
        neutral_dir = target_dir / f"{module_id}.2048"
        neutral_dir.mkdir()
        shutil.copy2(
            source_dir / f"{module_id}.2048" / "texture_00.png",
            neutral_dir / "texture_00.png",
        )
    module_dir = module_root / "body-balanced-v1"
    outside = (
        module_root / "escaped.png"
        if path_kind == "traversal"
        else tmp_path / "absolute.png"
    )
    outside.write_bytes(
        (module_dir / "body-balanced-v1.2048" / "texture_00.png").read_bytes()
    )
    contract_path = module_dir / "模块.json"
    contract = json.loads(contract_path.read_text(encoding="utf-8"))
    contract["files"]["neutralTexture"] = (
        "../escaped.png" if path_kind == "traversal" else str(outside.resolve())
    )
    contract_path.write_text(json.dumps(contract), encoding="utf-8")
    contract_sha = hashlib.sha256(contract_path.read_bytes()).hexdigest()
    index = json.loads(json.dumps(GUIDE_INDEX))
    _guide_entry_from(index)["moduleContractSha256"] = contract_sha
    request = replace(_request(), body_module_contract_sha256=contract_sha)
    monkeypatch.setattr(pipelines, "_MODULE_ROOT", module_root)

    with pytest.raises(ContractError, match="module directory"):
        render_texture_atlas(request, FakeImageClient(_valid_provider()), index)

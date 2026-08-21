from __future__ import annotations

import base64
import hashlib
import struct
import sys
from time import monotonic, sleep
import zlib
from pathlib import Path
from threading import Event, Lock

from fastapi.testclient import TestClient

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from photo_avatar_backend import app as app_module  # noqa: E402
from photo_avatar_backend.audit import AuditContextV1  # noqa: E402
from photo_avatar_backend.app import PipelineRunner, create_app  # noqa: E402
from photo_avatar_backend.config import BackendConfig  # noqa: E402
from photo_avatar_backend.contracts import ContractError, StepRequest  # noqa: E402
from photo_avatar_backend.job_store import JobStore  # noqa: E402
from photo_avatar_backend.pipelines import TextureArtifact  # noqa: E402
from photo_avatar_backend.pixel_avatar import PixelAvatarArtifact  # noqa: E402
from photo_avatar_backend.pixel_audit import (  # noqa: E402
    PixelAlphaReportV1,
    PixelAvatarAuditV1,
)


AUTH = {"Authorization": "Bearer desktop-only-token"}


def _png_bytes(width: int = 256, height: int = 256) -> bytes:
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


def _request() -> dict[str, object]:
    image = _png_bytes()
    return {
        "sessionId": "desktop-session-1",
        "revision": 0,
        "providerSessionId": None,
        "step": "analyzeIdentity",
        "attempt": 1,
        "consentVersion": "photo-avatar-third-party-ai-lk888-no-delete-v2",
        "sourceImages": [
            {
                "sourceId": "source-0",
                "pngBase64": base64.b64encode(image).decode("ascii"),
                "sha256": hashlib.sha256(image).hexdigest(),
                "width": 256,
                "height": 256,
            }
        ],
        "profile": None,
        "bodyModuleContractSha256": None,
        "modification": None,
        "lockedTraits": [],
    }


def _profile() -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "species": "cat",
        "style": "animated-film-soft-v1",
        "bodyModuleId": "body-balanced-v1",
        "bodyModuleSource": "user",
        "traits": [
            {
                "key": "faceShape",
                "value": "round",
                "source": "user",
                "evidencePhotoIds": ["source-0"],
            },
            {
                "key": "bodyType",
                "value": "balanced",
                "source": "user",
                "evidencePhotoIds": ["source-0"],
            },
        ],
        "completionSummary": [],
    }


def _complete_profile() -> dict[str, object]:
    profile = _profile()
    known = {trait["key"] for trait in profile["traits"]}
    profile["traits"].extend(
        {
            "key": key,
            "value": f"completed-{key}",
            "source": "ai-completed",
            "evidencePhotoIds": [],
        }
        for key in (
            "faceProportions",
            "furColors",
            "markings",
            "eyeShape",
            "eyeColor",
            "earShape",
            "tail",
            "signatureMarks",
            "temperament",
        )
        if key not in known
    )
    profile["completionSummary"] = [
        trait["key"] for trait in profile["traits"] if trait["source"] == "ai-completed"
    ]
    return profile


def _completion_payload() -> dict[str, object]:
    payload = _request()
    payload.update(
        providerSessionId="provider-completion-1",
        step="completeAppearance",
        sourceImages=[],
        profile=_profile(),
    )
    return payload


def _completion_request() -> StepRequest:
    return StepRequest.parse(_completion_payload())


def _texture_request() -> dict[str, object]:
    payload = _request()
    payload.update(
        providerSessionId="provider-texture-1",
        step="renderTextureAtlas",
        profile=_complete_profile(),
        bodyModuleContractSha256="a" * 64,
    )
    return payload


def _wait_for(check, timeout: float = 1.0) -> None:
    deadline = monotonic() + timeout
    while monotonic() < deadline:
        if check():
            return
        sleep(0.01)
    assert check()


class BlockingIdentityRunner:
    def __init__(self) -> None:
        self.calls = 0
        self.started = Event()
        self.release = Event()
        self.finished = Event()

    def run(self, request):
        try:
            self.calls += 1
            self.started.set()
            assert self.release.wait(timeout=2)
            return _profile()
        finally:
            self.finished.set()


class BlockingRunner:
    def __init__(self) -> None:
        self.calls = 0
        self._lock = Lock()
        self.release = Event()

    def run(self, request):
        with self._lock:
            self.calls += 1
        assert self.release.wait(timeout=2)
        return _profile()


class TextureRunner:
    def __init__(self) -> None:
        self.png = b"\x89PNG\r\n\x1a\ntexture-artifact"

    def run(self, request):
        return TextureArtifact(
            png=self.png,
            sha256=hashlib.sha256(self.png).hexdigest(),
            provider_task_id="lk888-texture-task",
            body_module_id="body-balanced-v1",
            body_module_contract_sha256="a" * 64,
            provider_raw_sha256="1" * 64,
            source_texture_sha256="2" * 64,
            source_alpha_sha256="3" * 64,
            work_canvas_sha256="4" * 64,
            region_map_sha256="5" * 64,
            composer_version="deterministic-alpha-v1",
            png_encoder_version="pillow-png-v1",
            coverage_report={"minimumChangeRatio": 0.95},
        )

    def audit_context(self, request):
        return AuditContextV1(
            provider_model="gpt-image-2",
            body_module_id="body-balanced-v1",
            module_contract_sha256="a" * 64,
            source_texture_sha256="2" * 64,
            source_alpha_sha256="3" * 64,
            work_canvas_sha256="4" * 64,
            region_map_sha256="5" * 64,
            composer_version="deterministic-alpha-v1",
            png_encoder_version="pillow-png-v1",
        )


class PixelRunner:
    def __init__(self) -> None:
        self.png = _png_bytes(1024, 1024)

    def run(self, request):
        sha256 = hashlib.sha256(self.png).hexdigest()
        return PixelAvatarArtifact(
            png=self.png,
            sha256=sha256,
            width=1024,
            height=1024,
            audit=PixelAvatarAuditV1(
                schema_version=1,
                session_id="desktop-session-1",
                revision=0,
                attempt=1,
                provider="lk888",
                provider_model="gpt-image-2",
                provider_task_id="108652999",
                style_profile_id="pixel-style-v1",
                style_profile_sha256="342d61eaf88eecba41bbb7a21c76c000aa16d6b86dce03ef570431f746e34830",
                reference_sha256="5ebbaece6553ffa450731660aa0d3fbb208d8f2761e48eabfe696bc20a39447a",
                prompt_template_version="pixel-style-v1-prompt-v1",
                identity_profile_sha256="3" * 64,
                provider_raw_sha256=sha256,
                normalized_sha256=sha256,
                width=1024,
                height=1024,
                alpha_report=PixelAlphaReportV1(
                    visible_pixels=1,
                    partial_alpha_pixels=0,
                    partial_alpha_ratio=0.0,
                    largest_component_pixels=1,
                    largest_component_share=1.0,
                    bounds_left=32,
                    bounds_top=32,
                    bounds_right=992,
                    bounds_bottom=992,
                    margin_left=32,
                    margin_top=32,
                    margin_right=32,
                    margin_bottom=32,
                ),
                privacy_policy_version="unverified",
                retention_policy="unverified",
                upstream_delete_api="unsupported",
                status="succeeded",
                error_code=None,
                created_at="2026-08-18T00:00:00+00:00",
                completed_at="2026-08-18T00:00:01+00:00",
            ),
        )

    def audit_context(self, request):
        return AuditContextV1(provider_model="gpt-image-2")


def _client(tmp_path: Path, runner) -> TestClient:
    config = BackendConfig(
        lk888_api_key="provider-secret",
        backend_token="desktop-only-token",
        state_dir=tmp_path / "state",
    )
    return TestClient(create_app(config, JobStore(config.state_dir, runner=runner)))


def test_routes_require_bearer_return_immediately_and_delete_reports_unsupported_upstream(
    tmp_path: Path,
):
    runner = BlockingIdentityRunner()
    with _client(tmp_path, runner) as client:
        assert client.get("/healthz").json() == {"status": "ok"}
        assert client.post("/v1/photo-avatar/steps", json=_request()).json() == {
            "code": "auth",
            "message": "unauthorized",
        }

        created = client.post("/v1/photo-avatar/steps", json=_request(), headers=AUTH)
        assert created.status_code == 200
        assert set(created.json()) == {"providerSessionId", "jobId"}
        assert runner.started.wait(timeout=1)
        assert runner.calls == 1

        running = client.get(f"/v1/photo-avatar/jobs/{created.json()['jobId']}", headers=AUTH)
        assert running.json() == {"state": "running", "result": None, "error": None}

        deleted = client.delete(
            f"/v1/photo-avatar/sessions/{created.json()['providerSessionId']}", headers=AUTH
        )
        assert deleted.json() == {
            "backendCleanup": "deleted",
            "upstreamCleanup": "unsupported",
            "provider": "lk888",
        }
        runner.release.set()
        assert runner.finished.wait(timeout=1)


def test_job_status_adapts_identity_result_to_frozen_rust_wire(tmp_path: Path):
    runner = BlockingIdentityRunner()
    with _client(tmp_path, runner) as client:
        created = client.post("/v1/photo-avatar/steps", json=_request(), headers=AUTH).json()
        assert runner.started.wait(timeout=1)
        runner.release.set()
        assert runner.finished.wait(timeout=1)

        response = client.get(f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH)

        assert response.json() == {
            "state": "succeeded",
            "result": {"resultType": "identity", "partialProfile": _profile()},
            "error": None,
        }


def test_failed_job_wire_preserves_the_safe_local_contract_message(tmp_path: Path):
    class ContractFailingRunner:
        def run(self, request: StepRequest) -> object:
            raise ContractError("pixel artifact partial alpha ratio exceeds 2 percent")

    with _client(tmp_path, ContractFailingRunner()) as client:
        created = client.post("/v1/photo-avatar/steps", json=_request(), headers=AUTH).json()
        _wait_for(
            lambda: client.get(
                f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH
            ).json()["state"]
            == "failed"
        )
        response = client.get(
            f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH
        )

    assert response.json() == {
        "state": "failed",
        "result": None,
        "error": {
            "code": "invalidInput",
            "message": "生成图片不符合像素素材要求，请重试。",
        },
    }


def test_invalid_json_shape_uses_safe_existing_error_contract(tmp_path: Path):
    with _client(tmp_path, BlockingIdentityRunner()) as client:
        response = client.post("/v1/photo-avatar/steps", json=[], headers=AUTH)

        assert response.status_code == 400
        assert response.json() == {"code": "invalidInput", "message": "invalid request"}


def test_completion_runner_and_http_poll_emit_strict_rust_completion_without_legacy_profile(
    monkeypatch, tmp_path: Path
):
    config = BackendConfig(lk888_api_key="provider-secret", backend_token="desktop-only-token")
    monkeypatch.setattr(app_module, "complete_appearance", lambda request, client: _complete_profile())

    result = PipelineRunner(config).run(_completion_request())

    expected = {
        "requestedTraitKeys": [],
        "completedTraits": [
            trait
            for trait in _complete_profile()["traits"]
            if trait["source"] == "ai-completed"
        ],
        "bodyModuleId": "body-balanced-v1",
        "bodyModuleSource": "user",
    }
    assert result == expected
    assert "profile" not in result
    with TestClient(create_app(config, JobStore(tmp_path / "state", runner=PipelineRunner(config)))) as client:
        created = client.post("/v1/photo-avatar/steps", json=_completion_payload(), headers=AUTH).json()
        _wait_for(
            lambda: client.get(f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH).json()[
                "state"
            ]
            == "succeeded"
        )
        wire = client.get(f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH).json()["result"]
    assert wire == {"resultType": "appearance", "completion": expected}
    assert "profile" not in wire


def test_texture_job_uses_request_origin_and_keeps_artifact_bearer_protected(tmp_path: Path):
    runner = TextureRunner()
    with _client(tmp_path, runner) as client:
        created = client.post("/v1/photo-avatar/steps", json=_texture_request(), headers=AUTH).json()
        _wait_for(
            lambda: client.get(f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH).json()[
                "state"
            ]
            == "succeeded"
        )
        poll = client.get(f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH).json()
        result = poll["result"]
        assert result["artifactUrl"].startswith("http://testserver/")
        assert result["sha256"] == hashlib.sha256(runner.png).hexdigest()
        assert result["audit"]["canonicalSha256"] == result["sha256"]
        assert result["audit"]["status"] == "succeeded"
        assert "pngBase64" not in result["audit"]
        assert (result["width"], result["height"]) == (2048, 2048)
        assert client.get(result["artifactUrl"]).status_code == 401
        artifact = client.get(result["artifactUrl"], headers=AUTH)
        assert artifact.content == runner.png
        assert hashlib.sha256(artifact.content).hexdigest() == result["sha256"]


def test_pixel_job_returns_pixel_avatar_result_without_live2d_audit_fields(tmp_path: Path):
    payload = _request()
    payload.update(
        route="pixel-v1",
        providerSessionId="provider-pixel-1",
        step="generatePixelAvatar",
        profile={
            "schemaVersion": 1,
            "species": "cat",
            "styleProfileId": "pixel-style-v1",
            "traits": [
                {
                    "key": "faceShape",
                    "value": "round",
                    "source": "user",
                    "evidencePhotoIds": ["source-0"],
                }
            ],
            "completionSummary": [],
        },
    )
    payload.pop("bodyModuleContractSha256")
    with _client(tmp_path, PixelRunner()) as client:
        created = client.post("/v1/photo-avatar/steps", json=payload, headers=AUTH).json()
        _wait_for(
            lambda: client.get(
                f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH
            ).json()["state"]
            == "succeeded"
        )
        result = client.get(
            f"/v1/photo-avatar/jobs/{created['jobId']}", headers=AUTH
        ).json()["result"]

    assert result["resultType"] == "pixelAvatar"
    assert result["audit"]["styleProfileId"] == "pixel-style-v1"
    assert "bodyModuleId" not in result["audit"]


def test_app_uses_two_worker_executor_and_runs_an_idempotent_job_once(tmp_path: Path):
    runner = BlockingRunner()
    with _client(tmp_path, runner) as client:
        requests = []
        for index in range(3):
            payload = _request()
            payload["sessionId"] = f"desktop-session-{index}"
            requests.append(client.post("/v1/photo-avatar/steps", json=payload, headers=AUTH).json())
        duplicate = client.post("/v1/photo-avatar/steps", json=_request(), headers=AUTH).json()
        assert duplicate["jobId"] == requests[1]["jobId"]
        _wait_for(lambda: runner.calls == 2)
        assert client.app.state.executor._max_workers == 2
        runner.release.set()
    assert not any(thread.name.startswith("photo-avatar-worker") for thread in __import__("threading").enumerate())


def test_unknown_route_uses_safe_existing_error_contract(tmp_path: Path):
    with _client(tmp_path, BlockingIdentityRunner()) as client:
        response = client.get("/not-a-route")

    assert response.status_code == 404
    assert response.json() == {"code": "invalidInput", "message": "request rejected"}

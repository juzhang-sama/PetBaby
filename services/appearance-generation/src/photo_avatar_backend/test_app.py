from __future__ import annotations

import base64
from dataclasses import replace
import hashlib
import struct
import sys
from time import monotonic, sleep
from urllib.parse import urlparse
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
from photo_avatar_backend.frame_pipeline import (  # noqa: E402
    FramePipelineError,
    FrameSequenceArtifact,
    MotionSource,
)
from photo_avatar_backend.job_store import JobStore  # noqa: E402
from photo_avatar_backend.pipelines import TextureArtifact  # noqa: E402
from photo_avatar_backend.pixel_avatar import PixelAvatarArtifact  # noqa: E402
from photo_avatar_backend.pixel_audit import (  # noqa: E402
    PixelAlphaReportV1,
    PixelAvatarAuditV2,
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
            audit=PixelAvatarAuditV2(
                schema_version=2,
                session_id="desktop-session-1",
                revision=0,
                attempt=1,
                provider="lk888",
                provider_model="gpt-image-2",
                provider_task_id="108652999",
                style_profile_id="pixel-style-v2-animation-ready",
                style_profile_sha256="2a48f382d0d0a579010ffae2ce90a7693d364a0cf64e5463e0ce7bf0291ee4ab",
                reference_sha256="75171817d27aee72439f373317ad0a3f43bdb2f8a76b0f8c55e24c306ac46c85",
                prompt_template_version="pixel-style-v2-animation-ready-prompt-v2",
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
                logical_grid_size=160,
                palette_color_limit=24,
                visible_color_count=1,
                quantize_method="maxcoverage",
                dither="none",
                protected_accent_slots=4,
                protected_accent_count=0,
                downsample="box",
                upsample="nearest",
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
        styleProfileId="pixel-style-v2-animation-ready",
        providerSessionId="provider-pixel-1",
        step="generatePixelAvatar",
        profile={
            "schemaVersion": 1,
            "species": "cat",
            "styleProfileId": "pixel-style-v2-animation-ready",
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
    assert result["audit"]["styleProfileId"] == "pixel-style-v2-animation-ready"
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


# ---------------------------------------------------------------- 写实风（frame-video-v1）


class FrameSequenceRunner:
    """写实风 `packFrameSequence` 的替身：返回一支 zip。这步不调 provider。"""

    def __init__(self, payload: bytes = b"PK\x03\x04frame-sequence-zip") -> None:
        self.payload = payload
        self.calls = 0

    def run(self, request):
        self.calls += 1
        return FrameSequenceArtifact(
            payload=self.payload,
            sha256=hashlib.sha256(self.payload).hexdigest(),
            frame_count=288,
            frame_duration_ms=42,
            frame_format="webp",
            overall_passed=True,
            failed_criteria=(),
        )

    def audit_context(self, request):
        return AuditContextV1(provider_model="seedance-2.0-guanfang")


def _frame_request(**overrides: object) -> dict[str, object]:
    payload: dict[str, object] = {
        "route": "frame-video-v1",
        "sessionId": "desktop-session-1",
        "revision": 0,
        "providerSessionId": "provider-frame-1",
        "step": "packFrameSequence",
        "attempt": 1,
        "consentVersion": "photo-avatar-third-party-ai-lk888-no-delete-v2",
        "sourceImages": [],
        "petId": "09-newcat",
        "displayName": "我的猫",
        "species": "cat",
    }
    payload.update(overrides)
    return payload


def _job_body(client: TestClient, job_id: str) -> dict[str, object]:
    body: dict[str, object] = {}

    def settled() -> bool:
        body.update(client.get(f"/v1/photo-avatar/jobs/{job_id}", headers=AUTH).json())
        return body["state"] != "running"

    _wait_for(settled)
    return body


def test_frame_sequence_job_returns_a_zip_artifact_served_as_zip(tmp_path: Path):
    runner = FrameSequenceRunner()
    with _client(tmp_path, runner) as client:
        created = client.post("/v1/photo-avatar/steps", json=_frame_request(), headers=AUTH)
        assert created.status_code == 200

        body = _job_body(client, created.json()["jobId"])
        assert body["state"] == "succeeded", body
        result = body["result"]

        # artifact 端点以前硬编码 image/png —— 写实风交付的是 zip，必须按扩展名给。
        # 必须在**同一个 client** 里取：`_client` 会新建 JobStore，而重启后
        # artifact 会按设计被删（job 结果不跨重启）。
        fetched = client.get(urlparse(str(result["artifactUrl"])).path, headers=AUTH)

    assert result["resultType"] == "frameSequence"
    assert result["sha256"] == hashlib.sha256(runner.payload).hexdigest()
    assert result["frameCount"] == 288
    assert result["frameDurationMs"] == 42
    assert result["frameFormat"] == "webp"
    assert result["overallPassed"] is True
    assert result["failedCriteria"] == []
    assert fetched.status_code == 200
    assert fetched.headers["content-type"] == "application/zip"
    assert fetched.content == runner.payload


def test_frame_sequence_wire_reports_failed_criteria_so_the_client_can_warn(tmp_path: Path):
    """判据 FAIL 不抛异常，但结论必须到得了客户端 —— 人工确认那步要提示「检测到异常」。"""

    class FailingRunner(FrameSequenceRunner):
        def run(self, request):
            artifact = super().run(request)
            return replace(
                artifact,
                overall_passed=False,
                failed_criteria=("3-帧间不闪烁", "1-尾巴完整"),
            )

    with _client(tmp_path, FailingRunner()) as client:
        created = client.post("/v1/photo-avatar/steps", json=_frame_request(), headers=AUTH)
        body = _job_body(client, created.json()["jobId"])

    assert body["state"] == "succeeded", body
    assert body["result"]["overallPassed"] is False
    assert body["result"]["failedCriteria"] == ["3-帧间不闪烁", "1-尾巴完整"]


def test_motion_source_step_reports_its_result_without_an_artifact(tmp_path: Path):
    """`generateMotionSource` **不交付字节**：mp4 留在服务侧 scratch。

    客户端要的只是「成了没有 / 是复用还是新跑」。这条 wire 形状就是客户端的全部输入 ——
    它没有 artifactUrl，所以任何「一个 job 一个 artifact」的假设在这里都不成立。
    """

    class MotionSourceRunner:
        def run(self, request):
            return MotionSource(
                out_dir=tmp_path / "scratch" / "provider-frame-1",
                video_path=tmp_path / "scratch" / "provider-frame-1" / "motion-source.mp4",
                master_path=None,
                first_frame_path=None,
                master_task_id=None,
                video_task_id=None,
                first_frame_scale=None,
                first_frame_left_margin=None,
                first_frame_right_margin=None,
                video_bytes=9_600_000,
                reused=True,
            )

        def audit_context(self, request):
            return AuditContextV1(provider_model="seedance-2.0-guanfang")

    with _client(tmp_path, MotionSourceRunner()) as client:
        created = client.post(
            "/v1/photo-avatar/steps",
            json=_frame_request(step="generateMotionSource", providerSessionId=None,
                                sourceImages=_request()["sourceImages"]),
            headers=AUTH,
        )
        assert created.status_code == 200
        body = _job_body(client, created.json()["jobId"])

    assert body["state"] == "succeeded", body
    result = body["result"]
    assert result["resultType"] == "motionSource"
    assert result["reused"] is True
    assert result["videoBytes"] == 9_600_000
    assert "artifactUrl" not in result
    # 动作也在 wire 上（这支 stub 一支都没做 → 空列表）
    assert result["actions"] == []


def test_motion_source_framing_failure_reaches_the_client_as_invalid_input(tmp_path: Path):
    """取景收敛全挂 = **照片的问题** → `invalidInput`（重试同一张只会再失败一次）。

    服务故障才是 `temporaryUnavailable`。两者混起来会让客户端要么白重试、
    要么把好照片劝退。
    """

    class UnfittableRunner:
        def run(self, request):
            raise FramePipelineError("取景收敛失败：请换一张正面坐姿、尾巴收拢的照片。",
                                     code="invalidInput")

        def audit_context(self, request):
            return AuditContextV1(provider_model="seedance-2.0-guanfang")

    with _client(tmp_path, UnfittableRunner()) as client:
        created = client.post(
            "/v1/photo-avatar/steps",
            json=_frame_request(step="generateMotionSource", providerSessionId=None,
                                sourceImages=_request()["sourceImages"]),
            headers=AUTH,
        )
        body = _job_body(client, created.json()["jobId"])

    assert body["state"] == "failed", body
    assert body["error"]["code"] == "invalidInput"


def test_motion_source_internal_failure_stays_retryable(tmp_path: Path):
    """默认码是 `temporaryUnavailable` —— 没指明原因时保守地当成可重试。"""

    class BrokenRunner:
        def run(self, request):
            raise FramePipelineError("上游对象存储 502")

    with _client(tmp_path, BrokenRunner()) as client:
        created = client.post(
            "/v1/photo-avatar/steps",
            json=_frame_request(step="generateMotionSource", providerSessionId=None,
                                sourceImages=_request()["sourceImages"]),
            headers=AUTH,
        )
        body = _job_body(client, created.json()["jobId"])

    assert body["state"] == "failed", body
    assert body["error"]["code"] == "temporaryUnavailable"


def test_frame_route_rejects_a_photo_on_the_pack_step(tmp_path: Path):
    """契约层就该拒掉，别让它变成实施层一个「悄悄忽略 sourceImages」的分支。"""
    with _client(tmp_path, FrameSequenceRunner()) as client:
        response = client.post(
            "/v1/photo-avatar/steps",
            json=_frame_request(sourceImages=_request()["sourceImages"]),
            headers=AUTH,
        )

    assert response.status_code == 400
    assert response.json() == {"code": "invalidInput", "message": "invalid request"}

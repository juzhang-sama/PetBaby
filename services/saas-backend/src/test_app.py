# -*- coding: utf-8 -*-
import time
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from src.app import create_app
from src.provider import GenerationResult
from src.storage import GenerationStorage
from src.analyzer import PetAnalyzer

PNG_BYTES = b"\x89PNG\r\n\x1a\nfake-image"
RESULT_BYTES = b"fake-png-result"


class FakeProvider:
    def __init__(self, image_bytes: bytes = RESULT_BYTES):
        self.image_bytes = image_bytes
        self.last_mimes: list[str] | None = None

    def generate(
        self,
        prompt,
        ref_images=None,
        mime="image/png",
        mimes=None,
        size="auto",
        poll_interval=5.0,
        max_wait=300.0,
        on_progress=None,
    ):
        self.last_mimes = mimes
        return GenerationResult(task_id="t1", image_bytes=self.image_bytes)


class FakeAnalyzer:
    def __init__(self, traits: dict | None = None):
        self.traits = traits
        self.landmarks: dict | None = None
        self.calls = 0

    def analyze(self, photos, species):
        self.calls += 1
        return self.traits

    def analyze_landmarks(self, photo, species):
        return self.landmarks


@pytest.fixture
def client(tmp_path: Path):
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        analyzer_factory=lambda: FakeAnalyzer(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
    )
    with TestClient(app) as test_client:
        yield test_client


def test_healthz(client: TestClient) -> None:
    resp = client.get("/healthz")
    assert resp.status_code == 200
    assert resp.json() == {"status": "ok"}


def test_cors_allows_webview_origin(client: TestClient) -> None:
    resp = client.options(
        "/api/v1/generations",
        headers={
            "Origin": "http://tauri.localhost",
            "Access-Control-Request-Method": "POST",
        },
    )
    assert resp.status_code == 200
    assert resp.headers.get("access-control-allow-origin") == "*"


def test_create_then_complete_and_download(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        files={"photo": ("cat.png", PNG_BYTES, "image/png")},
        data={"species": "cat"},
    )
    assert resp.status_code == 202
    body = resp.json()
    assert body["status"] == "queued"
    job_id = body["jobId"]

    status = {"status": "queued"}
    for _ in range(200):
        status = client.get(f"/api/v1/generations/{job_id}").json()
        if status["status"] in ("completed", "failed"):
            break
        time.sleep(0.02)
    assert status["status"] == "completed"
    assert status["resultAvailable"] is True

    result = client.get(f"/api/v1/generations/{job_id}/result")
    assert result.status_code == 200
    assert result.content == RESULT_BYTES


def test_multi_photo_accepted_and_all_stored(tmp_path: Path) -> None:
    provider = FakeProvider()
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: provider,
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
        analyzer_factory=lambda: FakeAnalyzer(),
    )
    with TestClient(app) as client:
        resp = client.post(
            "/api/v1/generations",
            files=[
                ("photos", ("front.png", PNG_BYTES, "image/png")),
                ("photos", ("side.jpg", b"\xff\xd8\xffjpeg", "image/jpeg")),
            ],
            data={"species": "cat"},
        )
        assert resp.status_code == 202
        job_id = resp.json()["jobId"]
        photos = client.app.state.storage.list_photos(job_id)
        assert len(photos) == 2
        assert {p["mime"] for p in photos} == {"image/png", "image/jpeg"}

        status = {"status": "queued"}
        for _ in range(200):
            status = client.get(f"/api/v1/generations/{job_id}").json()
            if status["status"] in ("completed", "failed"):
                break
            time.sleep(0.02)
        assert status["status"] == "completed"
        assert provider.last_mimes == ["image/png", "image/jpeg"]


def test_more_than_three_photos_rejected(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        files=[
            ("photos", ("1.png", PNG_BYTES, "image/png")),
            ("photos", ("2.png", PNG_BYTES, "image/png")),
            ("photos", ("3.png", PNG_BYTES, "image/png")),
            ("photos", ("4.png", PNG_BYTES, "image/png")),
        ],
        data={"species": "cat"},
    )
    assert resp.status_code == 422
    assert "at most 3" in resp.json()["detail"]


def test_legacy_single_photo_field_still_works(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        files={"photo": ("cat.png", PNG_BYTES, "image/png")},
        data={"species": "dog"},
    )
    assert resp.status_code == 202


def test_analyzer_traits_flow_into_prompt(tmp_path: Path) -> None:
    traits = {
        "species": "cat",
        "fur_colors": ["white"],
        "pattern": "solid",
        "ears": "pointed",
        "eye_color": "green",
        "face_notes": "round amber eyes, pink nose",
    }
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
        analyzer_factory=lambda: FakeAnalyzer(traits),
    )
    with TestClient(app) as client:
        resp = client.post(
            "/api/v1/generations",
            files={"photo": ("cat.png", PNG_BYTES, "image/png")},
            data={"species": "cat"},
        )
        assert resp.status_code == 202
        body = resp.json()
        assert body["traits"] == traits
        job = client.app.state.storage.get_job(body["jobId"])
        assert job is not None
        assert "main fur colors: white" in job["prompt"]
        assert "face details: round amber eyes, pink nose" in job["prompt"]


def test_analyzer_failure_falls_back_to_reference_prompt(tmp_path: Path) -> None:
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
        analyzer_factory=lambda: FakeAnalyzer(None),
    )
    with TestClient(app) as client:
        resp = client.post(
            "/api/v1/generations",
            files={"photo": ("cat.png", PNG_BYTES, "image/png")},
            data={"species": "cat"},
        )
        assert resp.status_code == 202
        assert resp.json()["traits"] is None
        job = client.app.state.storage.get_job(resp.json()["jobId"])
        assert job is not None
        assert "High fidelity to the reference" in job["prompt"]


def test_3d_style_flows_into_prompt(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        files={"photo": ("cat.png", PNG_BYTES, "image/png")},
        data={"species": "cat", "style": "3d"},
    )
    assert resp.status_code == 202
    body = resp.json()
    assert body["style"] == "3d"
    job = client.app.state.storage.get_job(body["jobId"])
    assert job is not None
    assert "3D rendered pet" in job["prompt"]


def test_3d_style_works_for_guided_generation(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        data={
            "species": "cat",
            "style": "3d",
            "traits": '{"body": "round", "color": "orange"}',
        },
    )
    assert resp.status_code == 202
    job = client.app.state.storage.get_job(resp.json()["jobId"])
    assert job is not None
    assert "3D rendered pet" in job["prompt"]


def test_invalid_style_rejected(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        files={"photo": ("cat.png", PNG_BYTES, "image/png")},
        data={"species": "cat", "style": "watercolor"},
    )
    assert resp.status_code == 422


def test_landmarks_endpoint_returns_boxes(tmp_path: Path) -> None:
    box = {"x": 0.2, "y": 0.3, "width": 0.1, "height": 0.08}
    landmarks = {
        "leftEye": box,
        "rightEye": box,
        "leftEar": box,
        "rightEar": box,
        "tail": box,
    }
    analyzer = FakeAnalyzer()
    analyzer.landmarks = landmarks
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
        analyzer_factory=lambda: analyzer,
    )
    with TestClient(app) as client:
        resp = client.post(
            "/api/v1/landmarks",
            files={"photo": ("cat.png", PNG_BYTES, "image/png")},
            data={"species": "cat"},
        )
        assert resp.status_code == 200
        assert resp.json()["landmarks"] == landmarks


def test_landmarks_endpoint_falls_back_to_null(tmp_path: Path) -> None:
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
        analyzer_factory=lambda: FakeAnalyzer(),
    )
    with TestClient(app) as client:
        resp = client.post(
            "/api/v1/landmarks",
            files={"photo": ("cat.png", PNG_BYTES, "image/png")},
            data={"species": "cat"},
        )
        assert resp.status_code == 200
        assert resp.json()["landmarks"] is None


def test_landmarks_endpoint_rejects_non_image(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/landmarks",
        files={"photo": ("cat.txt", b"hello", "text/plain")},
        data={"species": "cat"},
    )
    assert resp.status_code == 422


def test_count_creates_multiple_jobs_and_analyzer_runs_once(tmp_path: Path) -> None:
    analyzer = FakeAnalyzer(
        {
            "species": "cat",
            "fur_colors": ["white"],
            "pattern": "solid",
            "ears": "pointed",
            "eye_color": "green",
            "face_notes": "round eyes",
        }
    )
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
        analyzer_factory=lambda: analyzer,
    )
    with TestClient(app) as client:
        resp = client.post(
            "/api/v1/generations",
            files={"photo": ("cat.png", PNG_BYTES, "image/png")},
            data={"species": "cat", "count": "3"},
        )
        assert resp.status_code == 202
        body = resp.json()
        assert len(body["jobIds"]) == 3
        assert body["jobId"] == body["jobIds"][0]
        assert analyzer.calls == 1
        for job_id in body["jobIds"]:
            job = client.app.state.storage.get_job(job_id)
            assert job is not None
            assert "face details: round eyes" in job["prompt"]
            assert len(client.app.state.storage.list_photos(job_id)) == 1


def test_count_out_of_range_rejected(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        files={"photo": ("cat.png", PNG_BYTES, "image/png")},
        data={"species": "cat", "count": "4"},
    )
    assert resp.status_code == 422
    assert "count must be between 1 and 3" in resp.json()["detail"]


def test_invalid_species_rejected(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        files={"photo": ("cat.png", PNG_BYTES, "image/png")},
        data={"species": "bird"},
    )
    assert resp.status_code == 422


def test_non_image_rejected(client: TestClient) -> None:
    resp = client.post(
        "/api/v1/generations",
        files={"photo": ("cat.txt", b"hello", "text/plain")},
        data={"species": "cat"},
    )
    assert resp.status_code == 422


def test_oversized_photo_rejected(client: TestClient) -> None:
    big = b"\x89PNG\r\n\x1a\n" + b"x" * (10 * 1024 * 1024)
    resp = client.post(
        "/api/v1/generations",
        files={"photo": ("cat.png", big, "image/png")},
        data={"species": "cat"},
    )
    assert resp.status_code == 413


def test_result_not_ready_returns_409(tmp_path: Path) -> None:
    class FailingProvider:
        def generate(self, *args, **kwargs):
            return GenerationResult(task_id="t1", error="boom")

    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FailingProvider(),
        analyzer_factory=lambda: FakeAnalyzer(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
    )
    with TestClient(app) as test_client:
        created = test_client.post(
            "/api/v1/generations",
            files={"photo": ("cat.png", PNG_BYTES, "image/png")},
            data={"species": "cat"},
        )
        job_id = created.json()["jobId"]
        for _ in range(200):
            status = test_client.get(f"/api/v1/generations/{job_id}").json()
            if status["status"] in ("completed", "failed"):
                break
            time.sleep(0.02)
        assert status["status"] == "failed"
        resp = test_client.get(f"/api/v1/generations/{job_id}/result")
        assert resp.status_code == 409


def test_missing_job_returns_404(client: TestClient) -> None:
    assert client.get("/api/v1/generations/nope").status_code == 404
    assert client.delete("/api/v1/generations/nope").status_code == 404


def test_auth_required_when_token_configured(tmp_path: Path) -> None:
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        analyzer_factory=lambda: FakeAnalyzer(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
        access_token="secret",
    )
    with TestClient(app) as client:
        assert client.get("/healthz").status_code == 200
        assert client.get("/api/v1/generations/nope").status_code == 401
        assert (
            client.get(
                "/api/v1/generations/nope",
                headers={"Authorization": "Bearer secret"},
            ).status_code
            == 404
        )
        resp = client.post(
            "/api/v1/generations",
            files={"photo": ("cat.png", PNG_BYTES, "image/png")},
            data={"species": "cat"},
            headers={"Authorization": "Bearer secret"},
        )
        assert resp.status_code == 202


def test_rate_limit_blocks_excess_requests(tmp_path: Path) -> None:
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        analyzer_factory=lambda: FakeAnalyzer(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
        rate_limit_per_minute=2,
    )
    with TestClient(app) as client:
        for _ in range(2):
            resp = client.post(
                "/api/v1/generations",
                files={"photo": ("cat.png", PNG_BYTES, "image/png")},
                data={"species": "cat"},
            )
            assert resp.status_code == 202
        resp = client.post(
            "/api/v1/generations",
            files={"photo": ("cat.png", PNG_BYTES, "image/png")},
            data={"species": "cat"},
        )
        assert resp.status_code == 429


def test_create_guided_generation_without_photo(tmp_path: Path) -> None:
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        analyzer_factory=lambda: FakeAnalyzer(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
    )
    with TestClient(app) as client:
        resp = client.post(
            "/api/v1/generations",
            data={
                "species": "cat",
                "traits": '{"body": "round", "color": "orange"}',
            },
        )
        assert resp.status_code == 202
        job_id = resp.json()["jobId"]

        status = {"status": "queued"}
        for _ in range(200):
            status = client.get(f"/api/v1/generations/{job_id}").json()
            if status["status"] in ("completed", "failed"):
                break
            time.sleep(0.02)
        assert status["status"] == "completed"
        job = client.app.state.storage.get_job(job_id)
        assert job is not None
        assert "orange" in job["prompt"]


def test_create_generation_requires_photo_or_traits(tmp_path: Path) -> None:
    app = create_app(
        storage=GenerationStorage(tmp_path / "test.db"),
        provider_factory=lambda: FakeProvider(),
        analyzer_factory=lambda: FakeAnalyzer(),
        data_dir=tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
    )
    with TestClient(app) as client:
        resp = client.post("/api/v1/generations", data={"species": "cat"})
        assert resp.status_code == 422


def test_delete_purges_job_and_files(client: TestClient) -> None:
    store = client.app.state.storage
    store.create_job("job-d", "dog")
    data_dir = client.app.state.data_dir
    photo_dir = data_dir / "photos" / "job-d"
    photo_dir.mkdir(parents=True)
    (photo_dir / "source.png").write_bytes(b"photo")

    resp = client.delete("/api/v1/generations/job-d")

    assert resp.status_code == 204
    assert store.get_job("job-d") is None
    assert not photo_dir.exists()

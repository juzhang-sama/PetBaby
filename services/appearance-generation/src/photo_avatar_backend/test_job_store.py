from __future__ import annotations

import hashlib
import json
import logging
from dataclasses import replace
from pathlib import Path
from threading import Event, Thread

import pytest

from .audit import AuditContextV1
from .contracts import ContractError, SourceImage, StepRequest
from .job_store import JobStore, JobStoreError
from .lk888_client import Lk888Error
from .pipelines import TextureArtifact


def _request(
    *,
    session_id: str = "desktop-session-1",
    provider_session_id: str | None = None,
    attempt: int = 1,
) -> StepRequest:
    photo = b"raw-photo-bytes-must-not-reach-disk"
    return StepRequest(
        session_id=session_id,
        revision=7,
        provider_session_id=provider_session_id,
        step="renderTextureAtlas",
        attempt=attempt,
        consent_version="photo-avatar-third-party-ai-lk888-no-delete-v2",
        source_images=(
            SourceImage(
                source_id="front-photo.png",
                png=photo,
                sha256=hashlib.sha256(photo).hexdigest(),
                width=256,
                height=256,
            ),
        ),
        profile={"sensitive": "prompt-like-profile-data"},
        body_module_contract_sha256="a" * 64,
        modification="user supplied change request",
        locked_traits=("faceShape",),
    )


class CountingRunner:
    def __init__(self) -> None:
        self.calls = 0

    def run(self, request: StepRequest) -> TextureArtifact:
        self.calls += 1
        return _artifact(f"lk888-task-{self.calls}")

    def audit_context(self, request: StepRequest) -> AuditContextV1:
        return AuditContextV1(
            provider_model="gpt-image-2",
            body_module_id="body-balanced-v1",
            module_contract_sha256="a" * 64,
            source_texture_sha256="b" * 64,
            source_alpha_sha256="c" * 64,
            work_canvas_sha256="d" * 64,
            region_map_sha256="e" * 64,
            composer_version="deterministic-alpha-v1",
            png_encoder_version="pillow-png-v1",
        )


class BlockingRunner(CountingRunner):
    def __init__(self) -> None:
        super().__init__()
        self.started = Event()
        self.release = Event()

    def run(self, request: StepRequest) -> TextureArtifact:
        self.calls += 1
        self.started.set()
        assert self.release.wait(timeout=5)
        return _artifact(f"lk888-task-{self.calls}")


class TaskReportingRunner(BlockingRunner):
    def run_with_task_reporter(self, request: StepRequest, report_task_id) -> TextureArtifact:
        self.calls += 1
        report_task_id("lk888-task-early")
        self.started.set()
        assert self.release.wait(timeout=5)
        return _artifact("lk888-task-early")


class DelayedTaskReporterRunner:
    def __init__(self) -> None:
        self.calls = 0
        self.submitted = Event()
        self.release_report = Event()

    def run_with_task_reporter(self, request: StepRequest, report_task_id) -> TextureArtifact:
        self.calls += 1
        self.submitted.set()
        assert self.release_report.wait(timeout=5)
        report_task_id("lk888-task-after-local-cleanup")
        return _artifact("lk888-task-after-local-cleanup")


def _artifact(task_id: str) -> TextureArtifact:
    png = b"\x89PNG\r\n\x1a\nartifact-data"
    return TextureArtifact(
        png=png,
        sha256=hashlib.sha256(png).hexdigest(),
        provider_task_id=task_id,
        body_module_id="body-balanced-v1",
        body_module_contract_sha256="a" * 64,
        provider_raw_sha256="1" * 64,
        source_texture_sha256="b" * 64,
        source_alpha_sha256="c" * 64,
        work_canvas_sha256="d" * 64,
        region_map_sha256="e" * 64,
        composer_version="deterministic-alpha-v1",
        png_encoder_version="pillow-png-v1",
        coverage_report={"minimumChangeRatio": 0.95},
    )


def test_terminal_audit_survives_artifact_and_session_cleanup(tmp_path: Path):
    store = JobStore(tmp_path, runner=CountingRunner())
    submitted = store.submit(_request())
    audit_path = tmp_path / "audits" / f"{submitted.job_id}.json"
    before = audit_path.read_bytes()

    store.delete_session(submitted.provider_session_id)

    assert audit_path.read_bytes() == before
    assert not list((tmp_path / "artifacts").glob("*.png"))
    assert b"pngBase64" not in before
    assert b"raw-photo-bytes" not in before


def test_failed_and_interrupted_jobs_write_terminal_audits(tmp_path: Path):
    class FailingRunner(CountingRunner):
        def run(self, request: StepRequest) -> TextureArtifact:
            raise Lk888Error("temporaryUnavailable", True, "fixture")

    failed_store = JobStore(tmp_path / "failed", runner=FailingRunner())
    failed = failed_store.submit(_request())
    failed_audit = json.loads(
        (tmp_path / "failed" / "audits" / f"{failed.job_id}.json").read_text(
            encoding="utf-8"
        )
    )
    assert failed_audit["status"] == "failed"
    assert failed_audit["errorCode"] == "temporaryUnavailable"
    assert failed_audit["canonicalSha256"] is None

    interrupted_root = tmp_path / "interrupted"
    first = JobStore(interrupted_root, runner=CountingRunner())
    reserved = first.reserve(_request(provider_session_id="provider-interrupted"))
    JobStore(interrupted_root, runner=CountingRunner())
    interrupted_audit = json.loads(
        (interrupted_root / "audits" / f"{reserved.job_id}.json").read_text(
            encoding="utf-8"
        )
    )
    assert interrupted_audit["status"] == "failed"
    assert interrupted_audit["errorCode"] == "temporaryUnavailable"


def test_restart_after_provider_success_recomposes_instead_of_accepting_raw(
    tmp_path: Path,
):
    provider_raw = b"opaque-provider-rgb-that-is-not-canonical"

    class CrashBeforeCanonicalRunner(CountingRunner):
        def run_with_task_reporter(self, request: StepRequest, report_task_id):
            self.calls += 1
            report_task_id("lk888-task-provider-succeeded")
            raise SystemExit("simulated crash before canonical write")

    class RecomposeRunner(CountingRunner):
        def run(self, request: StepRequest) -> TextureArtifact:
            self.calls += 1
            assert request.source_images[0].png == b"raw-photo-bytes-must-not-reach-disk"
            return replace(
                _artifact("lk888-task-recomposed"),
                provider_raw_sha256=hashlib.sha256(provider_raw).hexdigest(),
            )

    request = _request(provider_session_id="provider-canonical-crash")
    crashed = JobStore(tmp_path, runner=CrashBeforeCanonicalRunner())
    reserved = crashed.reserve(request)

    with pytest.raises(SystemExit, match="before canonical write"):
        crashed.run_reserved(reserved.job_id)

    persisted = json.loads(
        (tmp_path / "jobs" / f"{reserved.job_id}.json").read_text(encoding="utf-8")
    )
    assert persisted["status"] == "running"
    assert persisted["lk888TaskId"] == "lk888-task-provider-succeeded"
    assert not list((tmp_path / "artifacts").glob("*.png"))

    runner = RecomposeRunner()
    restarted = JobStore(tmp_path, runner=runner)
    assert restarted.status(reserved.job_id).status == "failed"
    recovered = restarted.submit(request)
    recovered_state = restarted.status(recovered.job_id)
    canonical = restarted.read_artifact(recovered_state.artifact_id or "")

    assert recovered.job_id != reserved.job_id
    assert runner.calls == 1
    assert canonical == _artifact("lk888-task-recomposed").png
    assert canonical != provider_raw
    assert recovered_state.audit is not None
    assert recovered_state.audit["providerRawSha256"] == hashlib.sha256(
        provider_raw
    ).hexdigest()
    assert recovered_state.audit["canonicalSha256"] == hashlib.sha256(
        canonical
    ).hexdigest()


def test_same_attempt_is_submitted_once_and_delete_removes_owned_artifacts(
    tmp_path: Path,
):
    runner = CountingRunner()
    store = JobStore(tmp_path, runner=runner)

    first = store.submit(_request())
    second = store.submit(_request())

    assert first.job_id == second.job_id
    assert first.provider_session_id == second.provider_session_id
    assert runner.calls == 1
    assert store.status(first.job_id).status == "succeeded"
    assert store.read_artifact(store.status(first.job_id).artifact_id or "") == _artifact(
        "lk888-task-1"
    ).png

    report = store.delete_session(first.provider_session_id)

    assert report.backend_cleanup == "deleted"
    assert report.upstream_cleanup == "unsupported"
    assert not list(tmp_path.rglob("*.png"))
    assert not list((tmp_path / "jobs").glob("*.json"))
    assert len(list((tmp_path / "tombstones").glob("*.json"))) == 1


def test_deleted_session_discards_a_late_runner_artifact(tmp_path: Path):
    runner = BlockingRunner()
    store = JobStore(tmp_path, runner=runner)
    request = _request(provider_session_id="provider-late")
    submitted = []
    thread = Thread(target=lambda: submitted.append(store.submit(request)))
    thread.start()
    assert runner.started.wait(timeout=2)

    assert store.delete_session("provider-late").backend_cleanup == "deleted"
    runner.release.set()
    thread.join(timeout=2)

    assert not thread.is_alive()
    assert runner.calls == 1
    assert not list(tmp_path.rglob("*.png"))
    assert store.status(submitted[0].job_id).status == "deleted"


def test_cancelled_job_discards_late_artifact(tmp_path: Path):
    runner = BlockingRunner()
    store = JobStore(tmp_path, runner=runner)
    request = _request(provider_session_id="provider-cancel")
    submitted = []
    thread = Thread(target=lambda: submitted.append(store.submit(request)))
    thread.start()
    assert runner.started.wait(timeout=2)

    job_state_path = next((tmp_path / "jobs").glob("*.json"))
    job_id = json.loads(job_state_path.read_text(encoding="utf-8"))["jobId"]
    assert store.status(job_id).status == "running"
    assert store.cancel(job_id).status == "cancelled"
    runner.release.set()
    thread.join(timeout=2)

    assert not thread.is_alive()
    assert not list(tmp_path.rglob("*.png"))
    assert store.status(job_id).status == "cancelled"


def test_provider_task_id_is_persisted_while_runner_is_still_active(tmp_path: Path):
    runner = TaskReportingRunner()
    store = JobStore(tmp_path, runner=runner)
    reserved = store.reserve(_request(provider_session_id="provider-task-report"))
    thread = Thread(target=lambda: store.run_reserved(reserved.job_id))
    thread.start()
    assert runner.started.wait(timeout=2)

    state_path = tmp_path / "jobs" / f"{reserved.job_id}.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    assert state["status"] == "running"
    assert state["lk888TaskId"] == "lk888-task-early"

    runner.release.set()
    thread.join(timeout=2)
    assert not thread.is_alive()
    assert store.status(reserved.job_id).status == "succeeded"


def test_cancel_racing_task_report_keeps_remote_task_id_in_job_audit(tmp_path: Path):
    runner = DelayedTaskReporterRunner()
    store = JobStore(tmp_path, runner=runner)
    reserved = store.reserve(_request(provider_session_id="provider-cancel-race"))
    thread = Thread(target=lambda: store.run_reserved(reserved.job_id))
    thread.start()
    assert runner.submitted.wait(timeout=2)

    assert store.cancel(reserved.job_id).status == "cancelled"
    runner.release_report.set()
    thread.join(timeout=2)

    assert not thread.is_alive()
    state_path = tmp_path / "jobs" / f"{reserved.job_id}.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    assert state["status"] == "cancelled"
    assert state["lk888TaskId"] == "lk888-task-after-local-cleanup"


def test_delete_racing_task_report_keeps_remote_task_id_in_tombstone(tmp_path: Path):
    runner = DelayedTaskReporterRunner()
    store = JobStore(tmp_path, runner=runner)
    reserved = store.reserve(_request(provider_session_id="provider-delete-race"))
    thread = Thread(target=lambda: store.run_reserved(reserved.job_id))
    thread.start()
    assert runner.submitted.wait(timeout=2)

    assert store.delete_session("provider-delete-race").backend_cleanup == "deleted"
    runner.release_report.set()
    thread.join(timeout=2)

    assert not thread.is_alive()
    assert not list((tmp_path / "jobs").glob("*.json"))
    tombstone_path = next((tmp_path / "tombstones").glob("*.json"))
    tombstone = json.loads(tombstone_path.read_text(encoding="utf-8"))
    assert tombstone["providerSessionId"] == "provider-delete-race"
    assert tombstone["lk888TaskIds"] == ["lk888-task-after-local-cleanup"]


def test_delete_is_session_scoped_and_repeated_delete_is_idempotent(tmp_path: Path):
    store = JobStore(tmp_path, runner=CountingRunner())
    first = store.submit(_request(session_id="desktop-one", provider_session_id="provider-one"))
    second = store.submit(_request(session_id="desktop-two", provider_session_id="provider-two"))

    first_artifact = store.status(first.job_id).artifact_id
    second_artifact = store.status(second.job_id).artifact_id
    assert first_artifact and second_artifact
    assert store.delete_session("provider-one").backend_cleanup == "deleted"
    assert store.delete_session("provider-one").backend_cleanup == "deleted"

    with pytest.raises(JobStoreError, match="not found"):
        store.read_artifact(first_artifact)
    assert store.read_artifact(second_artifact) == _artifact("lk888-task-2").png
    assert store.status(second.job_id).status == "succeeded"


@pytest.mark.parametrize("unsafe_id", ("../escape", "..\\escape", "/absolute", ""))
def test_untrusted_identifiers_fail_closed(tmp_path: Path, unsafe_id: str):
    store = JobStore(tmp_path, runner=CountingRunner())

    with pytest.raises(JobStoreError, match="invalid"):
        store.status(unsafe_id)
    with pytest.raises(JobStoreError, match="invalid"):
        store.cancel(unsafe_id)
    with pytest.raises(JobStoreError, match="invalid"):
        store.delete_session(unsafe_id)
    with pytest.raises(JobStoreError, match="invalid"):
        store.read_artifact(unsafe_id)


def test_state_is_metadata_only_and_never_serializes_source_photos_or_prompt_data(
    tmp_path: Path,
):
    store = JobStore(tmp_path, runner=CountingRunner())
    submitted = store.submit(_request(provider_session_id="provider-private"))

    state_files = list((tmp_path / "jobs").glob("*.json"))
    assert len(state_files) == 1
    state = json.loads(state_files[0].read_text(encoding="utf-8"))
    assert set(state) == {
        "artifact",
        "auditContext",
        "createdAt",
        "error",
        "jobId",
        "lk888TaskId",
        "providerSessionId",
        "sessionId",
        "revision",
        "step",
        "attempt",
        "status",
        "updatedAt",
    }
    assert "raw-photo-bytes" not in json.dumps(state)
    assert "prompt-like-profile-data" not in json.dumps(state)
    assert state["jobId"] == submitted.job_id
    serialized = state_files[0].read_text(encoding="utf-8")
    for forbidden in (
        "raw-photo-bytes",
        "front-photo.png",
        "prompt-like-profile-data",
        "user supplied change request",
        "pngBase64",
        "sourceImages",
    ):
        assert forbidden not in serialized


def test_runner_failure_is_recorded_as_safe_error_without_artifact(tmp_path: Path):
    class FailingRunner:
        def run(self, request: StepRequest) -> object:
            raise RuntimeError("provider exploded")

    store = JobStore(tmp_path, runner=FailingRunner())
    submitted = store.submit(_request(provider_session_id="provider-failed"))

    state = store.status(submitted.job_id)
    assert state.status == "failed"
    assert state.error == {"code": "temporaryUnavailable", "message": "provider failed"}
    assert state.artifact_id is None
    assert not list(tmp_path.rglob("*.png"))


def test_storage_failure_records_local_storage_error_not_provider(tmp_path: Path):
    class StorageFailingRunner:
        def run(self, request: StepRequest) -> object:
            raise OSError("job file write failed")

    store = JobStore(tmp_path, runner=StorageFailingRunner())
    submitted = store.submit(_request(provider_session_id="provider-storage-failed"))

    state = store.status(submitted.job_id)
    assert state.status == "failed"
    assert state.error == {"code": "localStorage", "message": "backend storage failed"}
    assert state.artifact_id is None


def test_contract_failure_is_non_retryable_and_keeps_only_a_safe_message(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
):
    class ContractFailingRunner:
        def __init__(self) -> None:
            self.calls = 0

        def run(self, request: StepRequest) -> object:
            self.calls += 1
            raise ContractError(
                "pixel artifact alpha margin is below 2 percent; "
                "provider=https://private.example; token=secret-token"
            )

    runner = ContractFailingRunner()
    with caplog.at_level(logging.WARNING, logger="photo_avatar_backend.job_store"):
        store = JobStore(tmp_path, runner=runner)
        submitted = store.submit(_request(provider_session_id="provider-contract-failed"))

    state = store.status(submitted.job_id)
    assert runner.calls == 1
    assert state.status == "failed"
    assert state.error == {
        "code": "invalidInput",
        "message": "生成图片不符合像素素材要求，请重试。",
    }
    assert state.artifact_id is None
    assert "pixel artifact alpha margin is below 2 percent" in caplog.text
    for private_value in ("https://private.example", "secret-token"):
        assert private_value not in caplog.text
        assert private_value not in next(tmp_path.rglob("*.json")).read_text(encoding="utf-8")


def test_lk888_failure_logs_detail_locally_but_persists_only_generic_error(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
):
    class FailingRunner:
        def run(self, request: StepRequest) -> object:
            raise Lk888Error(
                "invalidInput",
                False,
                "provider rejected request",
                diagnostic=(
                    "code=bad_request;type=invalid_request_error;"
                    "param=response_format.json_schema;tags=json_schema,invalid"
                ),
            )

    with caplog.at_level(logging.WARNING, logger="photo_avatar_backend.job_store"):
        store = JobStore(tmp_path, runner=FailingRunner())
        submitted = store.submit(_request(provider_session_id="provider-failed"))

    messages = [record.getMessage() for record in caplog.records]
    assert messages == [
        "code=invalidInput retryable=False diagnostic=code=bad_request;"
        "type=invalid_request_error;param=response_format.json_schema;"
        "tags=json_schema,invalid"
    ]
    assert store.status(submitted.job_id).error == {
        "code": "invalidInput",
        "message": "provider failed",
    }
    serialized = next(tmp_path.rglob("*.json")).read_text(encoding="utf-8")
    for private_value in (
        "private prompt",
        "data:image/png;base64,PRIVATE_IMAGE",
        "Authorization: Bearer private-token",
    ):
        assert private_value not in serialized
        assert private_value not in caplog.text


def test_invalid_runner_artifact_becomes_failed_and_same_attempt_can_retry(
    tmp_path: Path,
):
    class InvalidArtifactRunner:
        def __init__(self) -> None:
            self.calls = 0

        def run(self, request: StepRequest) -> TextureArtifact:
            self.calls += 1
            artifact = _artifact("lk888-invalid")
            return replace(artifact, sha256="0" * 64)

    invalid = InvalidArtifactRunner()
    request = _request(provider_session_id="provider-invalid-artifact")
    first_store = JobStore(tmp_path, runner=invalid)

    failed = first_store.submit(request)

    assert first_store.status(failed.job_id).status == "failed"
    assert first_store.status(failed.job_id).artifact_id is None
    assert not list(tmp_path.rglob("*.png"))
    retry = CountingRunner()
    second_store = JobStore(tmp_path, runner=retry)
    succeeded = second_store.submit(request)
    assert retry.calls == 1
    assert succeeded.job_id != failed.job_id
    assert second_store.status(succeeded.job_id).status == "succeeded"


def test_restart_recovers_job_idempotency_artifact_and_session_cleanup(tmp_path: Path):
    request = _request(provider_session_id="provider-restart")
    first_runner = CountingRunner()
    first_store = JobStore(tmp_path, runner=first_runner)
    first = first_store.submit(request)
    artifact_id = first_store.status(first.job_id).artifact_id
    assert artifact_id is not None

    second_runner = CountingRunner()
    second_store = JobStore(tmp_path, runner=second_runner)
    repeated = second_store.submit(request)

    assert repeated == first
    assert second_runner.calls == 0
    assert second_store.status(first.job_id).status == "succeeded"
    assert second_store.read_artifact(artifact_id) == _artifact("lk888-task-1").png
    assert second_store.delete_session("provider-restart").backend_cleanup == "deleted"
    with pytest.raises(JobStoreError, match="not found"):
        second_store.read_artifact(artifact_id)


def test_deleted_session_tombstone_survives_restart_and_blocks_resubmit(tmp_path: Path):
    request = _request(provider_session_id="provider-deleted-restart")
    first_store = JobStore(tmp_path, runner=CountingRunner())
    first_store.submit(request)
    assert first_store.delete_session(request.provider_session_id or "").backend_cleanup == "deleted"

    retry_runner = CountingRunner()
    second_store = JobStore(tmp_path, runner=retry_runner)

    with pytest.raises(JobStoreError, match="deleted"):
        second_store.submit(request)
    assert retry_runner.calls == 0
    assert second_store.delete_session("provider-deleted-restart").backend_cleanup == "deleted"


def test_restart_recovers_from_delete_tombstone_crash_window(tmp_path: Path):
    deleted_request = _request(provider_session_id="provider-delete-crash")
    other_request = _request(
        session_id="desktop-other",
        provider_session_id="provider-other",
    )
    first_store = JobStore(tmp_path, runner=CountingRunner())
    deleted = first_store.submit(deleted_request)
    other = first_store.submit(other_request)
    deleted_artifact = first_store.status(deleted.job_id).artifact_id
    assert deleted_artifact is not None

    tombstones_dir = tmp_path / "tombstones"
    tombstones_dir.mkdir(exist_ok=True)
    (tombstones_dir / ("d" * 32 + ".json")).write_text(
        json.dumps(
            {
                "createdAt": "2026-08-16T00:00:00+00:00",
                "providerSessionId": "provider-delete-crash",
            }
        ),
        encoding="utf-8",
    )

    retry_runner = CountingRunner()
    second_store = JobStore(tmp_path, runner=retry_runner)

    with pytest.raises(JobStoreError, match="deleted"):
        second_store.submit(deleted_request)
    assert retry_runner.calls == 0
    with pytest.raises(JobStoreError, match="not found"):
        second_store.read_artifact(deleted_artifact)
    assert second_store.status(other.job_id).status == "succeeded"
    assert second_store.read_artifact(
        first_store.status(other.job_id).artifact_id or ""
    )
    assert not (tmp_path / "artifacts" / f"{deleted_artifact}.png").exists()
    assert not list(
        tmp_path.joinpath("jobs").glob("*.json")
    ) or all(
        json.loads(path.read_text(encoding="utf-8"))["providerSessionId"]
        != "provider-delete-crash"
        for path in (tmp_path / "jobs").glob("*.json")
    )


@pytest.mark.parametrize("result", (None, {"not": "an artifact"}), ids=("none", "other"))
def test_non_texture_runner_result_is_failed_and_same_attempt_can_retry(
    tmp_path: Path, result: object
):
    class EmptyRunner:
        def __init__(self) -> None:
            self.calls = 0

        def run(self, request: StepRequest) -> object:
            self.calls += 1
            return result

    result_name = "none" if result is None else "other"
    request = _request(provider_session_id=f"provider-empty-result-{result_name}")
    empty = EmptyRunner()
    failed_store = JobStore(tmp_path, runner=empty)
    failed = failed_store.submit(request)

    state = failed_store.status(failed.job_id)
    assert state.status == "failed"
    assert state.error == {"code": "invalidArtifact", "message": "provider returned invalid result"}
    assert state.artifact_id is None
    assert not list(tmp_path.rglob("*.png"))

    retry_runner = CountingRunner()
    retry_store = JobStore(tmp_path, runner=retry_runner)
    succeeded = retry_store.submit(request)
    assert retry_runner.calls == 1
    assert succeeded.job_id != failed.job_id
    assert retry_store.status(succeeded.job_id).status == "succeeded"


def test_restart_rejects_unknown_tombstone_fields(tmp_path: Path):
    provider_session_id = "provider-invalid-tombstone"
    store = JobStore(tmp_path, runner=CountingRunner())
    store.delete_session(provider_session_id)
    tombstone_path = next((tmp_path / "tombstones").glob("*.json"))
    tombstone = json.loads(tombstone_path.read_text(encoding="utf-8"))
    tombstone["unexpected"] = "must be rejected"
    tombstone_path.write_text(json.dumps(tombstone), encoding="utf-8")

    with pytest.raises(JobStoreError, match="tombstone"):
        JobStore(tmp_path, runner=CountingRunner())


def test_restart_rejects_tombstone_path_traversal_and_duplicates(tmp_path: Path):
    provider_session_id = "provider-tombstone-path"
    store = JobStore(tmp_path, runner=CountingRunner())
    store.delete_session(provider_session_id)
    tombstone_path = next((tmp_path / "tombstones").glob("*.json"))
    tombstone = json.loads(tombstone_path.read_text(encoding="utf-8"))
    tombstone["providerSessionId"] = "../escape"
    tombstone_path.write_text(json.dumps(tombstone), encoding="utf-8")
    with pytest.raises(JobStoreError, match="provider session"):
        JobStore(tmp_path, runner=CountingRunner())

    tombstone["providerSessionId"] = provider_session_id
    tombstone_path.write_text(json.dumps(tombstone), encoding="utf-8")
    duplicate_path = tmp_path / "tombstones" / "duplicate.json"
    duplicate_path.write_text(json.dumps(tombstone), encoding="utf-8")
    with pytest.raises(JobStoreError, match="tombstone"):
        JobStore(tmp_path, runner=CountingRunner())


def test_restart_rejects_unknown_or_malformed_state_json(tmp_path: Path):
    jobs_dir = tmp_path / "jobs"
    jobs_dir.mkdir()
    state = {
        "artifact": None,
        "createdAt": "2026-08-16T00:00:00+00:00",
        "error": None,
        "jobId": "job-invalid-state",
        "lk888TaskId": None,
        "providerSessionId": "provider-invalid-state",
        "sessionId": "desktop-session-1",
        "revision": 7,
        "step": "renderTextureAtlas",
        "attempt": 1,
        "status": "succeeded",
        "updatedAt": "2026-08-16T00:00:00+00:00",
        "unexpected": "must be rejected",
    }
    (jobs_dir / "job-invalid-state.json").write_text(
        json.dumps(state), encoding="utf-8"
    )

    with pytest.raises(JobStoreError, match="state"):
        JobStore(tmp_path, runner=CountingRunner())


def test_restart_rejects_artifact_path_escape(tmp_path: Path):
    request = _request(provider_session_id="provider-path-escape")
    store = JobStore(tmp_path, runner=CountingRunner())
    submitted = store.submit(request)
    state_path = next((tmp_path / "jobs").glob("*.json"))
    state = json.loads(state_path.read_text(encoding="utf-8"))
    state["artifact"]["path"] = "artifacts/../escaped.png"
    state_path.write_text(json.dumps(state), encoding="utf-8")

    with pytest.raises(JobStoreError, match="artifact"):
        JobStore(tmp_path, runner=CountingRunner())


def test_restart_rejects_conflicting_idempotency_records(tmp_path: Path):
    request = _request(provider_session_id="provider-conflict")
    store = JobStore(tmp_path, runner=CountingRunner())
    store.submit(request)
    original = next((tmp_path / "jobs").glob("*.json"))
    duplicate = tmp_path / "jobs" / "duplicate-job.json"
    state = json.loads(original.read_text(encoding="utf-8"))
    state["jobId"] = "duplicate-job"
    duplicate.write_text(json.dumps(state), encoding="utf-8")

    with pytest.raises(JobStoreError, match="idempotency"):
        JobStore(tmp_path, runner=CountingRunner())


def test_reserved_job_runs_once_in_background_and_keeps_identity_result_only_in_memory(
    tmp_path: Path,
):
    class BlockingIdentityRunner:
        def __init__(self) -> None:
            self.calls = 0
            self.started = Event()
            self.release = Event()

        def run(self, request: StepRequest) -> dict[str, object]:
            self.calls += 1
            self.started.set()
            assert self.release.wait(timeout=2)
            return {"identity": "current-process-only"}

    runner = BlockingIdentityRunner()
    store = JobStore(tmp_path, runner=runner)
    request = _request(provider_session_id="provider-reserved")
    request = replace(request, step="analyzeIdentity", provider_session_id=None)

    reserved = store.reserve(request)
    thread = Thread(target=lambda: store.run_reserved(reserved.job_id))
    thread.start()
    assert runner.started.wait(timeout=1)
    assert store.status(reserved.job_id).status == "running"
    assert runner.calls == 1

    runner.release.set()
    thread.join(timeout=2)
    assert not thread.is_alive()
    assert store.status(reserved.job_id).result == {"identity": "current-process-only"}
    assert "current-process-only" not in next((tmp_path / "jobs").glob("*.json")).read_text(
        encoding="utf-8"
    )

    restarted = JobStore(tmp_path, runner=CountingRunner())
    state = restarted.status(reserved.job_id)
    assert state.status == "failed"
    assert state.result is None
    assert state.error == {
        "code": "temporaryUnavailable",
        "message": "job result unavailable after restart",
    }


def test_restart_clears_artifact_when_non_final_success_is_invalidated(tmp_path: Path):
    store = JobStore(tmp_path, runner=CountingRunner())
    request = replace(
        _request(provider_session_id="provider-non-final-artifact"),
        step="generatePixelAvatar",
    )
    submitted = store.submit(request)
    assert store.status(submitted.job_id).status == "succeeded"
    assert list((tmp_path / "artifacts").glob("*.png"))

    restarted = JobStore(tmp_path, runner=CountingRunner())
    state = restarted.status(submitted.job_id)
    assert state.status == "failed"
    assert state.artifact_id is None
    assert state.artifact_sha256 is None
    assert not list((tmp_path / "artifacts").glob("*.png"))

    restarted_again = JobStore(tmp_path, runner=CountingRunner())
    assert restarted_again.status(submitted.job_id).status == "failed"


def test_restart_migrates_failed_job_with_stale_artifact(tmp_path: Path):
    store = JobStore(tmp_path, runner=CountingRunner())
    request = replace(
        _request(provider_session_id="provider-stale-artifact"),
        step="generatePixelAvatar",
    )
    submitted = store.submit(request)
    state_path = tmp_path / "jobs" / f"{submitted.job_id}.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    state["status"] = "failed"
    state["error"] = {
        "code": "temporaryUnavailable",
        "message": "job result unavailable after restart",
    }
    state_path.write_text(json.dumps(state), encoding="utf-8")

    restarted = JobStore(tmp_path, runner=CountingRunner())
    recovered = restarted.status(submitted.job_id)
    assert recovered.status == "failed"
    assert recovered.artifact_id is None
    assert recovered.artifact_sha256 is None
    assert not list((tmp_path / "artifacts").glob("*.png"))
    persisted = json.loads(state_path.read_text(encoding="utf-8"))
    assert persisted["artifact"] is None

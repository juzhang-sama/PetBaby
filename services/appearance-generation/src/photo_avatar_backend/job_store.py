"""Idempotent, local-only storage for photo-avatar backend jobs.

The store deliberately keeps request data only in memory for the one runner
invocation.  Its state directory contains a small deletion-safe job ledger and
randomly named generated PNG artifacts, never customer photo bytes or prompts.
"""

from __future__ import annotations

import copy
from dataclasses import dataclass
from datetime import UTC, datetime
import hashlib
import json
import logging
from pathlib import Path
from threading import RLock
from typing import Any
from uuid import uuid4

from .audit import (
    AttemptAuditV1,
    AuditContextV1,
    AuditContractError,
    SemanticAtlasAuditV1,
)
from .contracts import ContractError, PixelStepRequest, StepRequest
from .lk888_client import Lk888Error
from .pixel_audit import PixelAuditError, parse_pixel_avatar_audit
from .pixel_avatar import PixelAvatarArtifact
from .pipelines import TextureArtifact


_LOGGER = logging.getLogger(__name__)
_LEGACY_STATE_FIELDS = frozenset(
    {
        "artifact",
        "attempt",
        "createdAt",
        "error",
        "jobId",
        "lk888TaskId",
        "providerSessionId",
        "revision",
        "sessionId",
        "status",
        "step",
        "updatedAt",
    }
)
_STATE_FIELDS = _LEGACY_STATE_FIELDS | {"auditContext"}
_ARTIFACT_FIELDS = frozenset({"path", "sha256"})
_ERROR_FIELDS = frozenset({"code", "message"})
_LEGACY_TOMBSTONE_FIELDS = frozenset({"createdAt", "providerSessionId"})
_TOMBSTONE_FIELDS = frozenset(
    {"createdAt", "providerSessionId", "lk888TaskIds"}
)
_STATUSES = frozenset({"running", "succeeded", "failed", "cancelled", "deleted"})
_STEPS = frozenset({"analyzeIdentity", "completeAppearance", "renderTextureAtlas", "generatePixelAvatar"})
_PIXEL_CONTRACT_MESSAGE = "生成图片不符合像素素材要求，请重试。"


class JobStoreError(ValueError):
    """Raised when an untrusted identifier or unavailable local resource is used."""


@dataclass(frozen=True)
class SubmittedJob:
    job_id: str
    provider_session_id: str


@dataclass(frozen=True)
class JobState:
    job_id: str
    provider_session_id: str
    step: str
    status: str
    error: dict[str, str] | None
    artifact_id: str | None
    artifact_sha256: str | None
    result: dict[str, object] | None
    audit: dict[str, object] | None

    def to_wire(self) -> dict[str, object]:
        return {
            "jobId": self.job_id,
            "providerSessionId": self.provider_session_id,
            "step": self.step,
            "status": self.status,
            "error": self.error,
            "artifactId": self.artifact_id,
            "artifactSha256": self.artifact_sha256,
            "result": self.result,
            "audit": self.audit,
        }


@dataclass(frozen=True)
class CancelReport:
    job_id: str
    status: str

    def to_wire(self) -> dict[str, str]:
        return {"jobId": self.job_id, "status": self.status}


@dataclass(frozen=True)
class CleanupReport:
    backend_cleanup: str
    upstream_cleanup: str = "unsupported"
    provider: str = "lk888"

    def to_wire(self) -> dict[str, str]:
        return {
            "backendCleanup": self.backend_cleanup,
            "upstreamCleanup": self.upstream_cleanup,
            "provider": self.provider,
        }


@dataclass
class _Job:
    job_id: str
    session_id: str
    revision: int
    step: str
    attempt: int
    provider_session_id: str
    status: str
    created_at: str
    updated_at: str
    audit_context: AuditContextV1
    error: dict[str, str] | None = None
    artifact_id: str | None = None
    artifact_path: Path | None = None
    artifact_sha256: str | None = None
    lk888_task_id: str | None = None
    result: dict[str, object] | None = None


class JobStore:
    """Run each request attempt at most once and own its local cleanup."""

    def __init__(self, state_dir: Path, *, runner: Any):
        self.state_dir = Path(state_dir).resolve()
        self.runner = runner
        self._jobs_dir = self.state_dir / "jobs"
        self._artifacts_dir = self.state_dir / "artifacts"
        self._audits_dir = self.state_dir / "audits"
        self._tombstones_dir = self.state_dir / "tombstones"
        self._lock = RLock()
        self._jobs: dict[str, _Job] = {}
        self._idempotency: dict[tuple[str, int, str, int], str] = {}
        self._session_jobs: dict[str, set[str]] = {}
        self._session_owner: dict[str, str] = {}
        self._deleted_sessions: set[str] = set()
        self._tombstones: dict[str, tuple[Path, str, set[str]]] = {}
        self._pending_requests: dict[str, StepRequest | PixelStepRequest] = {}
        self._active_jobs: set[str] = set()
        self._load_tombstones()
        self._load_state()

    def submit(self, request: StepRequest | PixelStepRequest) -> SubmittedJob:
        """Run a step once for its exact session/revision/step/attempt key."""

        submitted = self.reserve(request)
        self.run_reserved(submitted.job_id)
        return submitted

    def reserve(self, request: StepRequest | PixelStepRequest) -> SubmittedJob:
        """Reserve a job for a later single background runner invocation."""

        _require_safe_id(request.session_id, "session id")
        try:
            audit_context = _invoke_audit_context(self.runner, request)
        except AuditContractError as exc:
            raise JobStoreError("runner returned an invalid audit context") from exc
        key = (request.session_id, request.revision, request.step, request.attempt)
        with self._lock:
            existing_id = self._idempotency.get(key)
            if existing_id is not None:
                existing = self._jobs[existing_id]
                if existing.status != "failed":
                    return SubmittedJob(existing.job_id, existing.provider_session_id)
                self._idempotency.pop(key)
                self._state_path(existing.job_id).unlink(missing_ok=True)

            provider_session_id = request.provider_session_id or uuid4().hex
            _require_safe_id(provider_session_id, "provider session id")
            if provider_session_id in self._deleted_sessions:
                raise JobStoreError("provider session has been deleted")
            owner = self._session_owner.get(provider_session_id)
            if owner is not None and owner != request.session_id:
                raise JobStoreError("provider session belongs to another session")
            self._session_owner[provider_session_id] = request.session_id

            now = _timestamp()
            job = _Job(
                job_id=uuid4().hex,
                session_id=request.session_id,
                revision=request.revision,
                step=request.step,
                attempt=request.attempt,
                provider_session_id=provider_session_id,
                status="running",
                created_at=now,
                updated_at=now,
                audit_context=audit_context,
            )
            self._jobs[job.job_id] = job
            self._idempotency[key] = job.job_id
            self._session_jobs.setdefault(provider_session_id, set()).add(job.job_id)
            self._pending_requests[job.job_id] = request
            self._persist(job)
            return SubmittedJob(job.job_id, provider_session_id)

    def run_reserved(self, job_id: str) -> None:
        """Run one reserved request once without persisting request or result data."""

        _require_safe_id(job_id, "job id")
        with self._lock:
            job = self._jobs.get(job_id)
            if job is None:
                raise JobStoreError("job not found")
            if job.status != "running" or job_id in self._active_jobs:
                return
            request = self._pending_requests.pop(job_id, None)
            if request is None:
                job.status = "failed"
                job.error = {
                    "code": "temporaryUnavailable",
                    "message": "job request unavailable",
                }
                job.updated_at = _timestamp()
                self._persist(job)
                self._ensure_terminal_audit(job)
                return
            self._active_jobs.add(job_id)

        try:
            result = _invoke_runner(
                self.runner,
                request,
                lambda task_id: self._record_lk888_task_id(job_id, task_id),
            )
        except Exception as exc:  # Runner errors are untrusted provider boundary data.
            if isinstance(exc, Lk888Error):
                _LOGGER.warning(
                    "code=%s retryable=%s diagnostic=%s",
                    exc.code,
                    exc.retryable,
                    exc.diagnostic or "unavailable",
                )
            elif isinstance(exc, ContractError):
                _LOGGER.warning("contract=%s", _safe_contract_diagnostic(exc))
            with self._lock:
                if job.status == "running":
                    job.status = "failed"
                    job.error = _safe_error(exc)
                    job.updated_at = _timestamp()
                    self._persist(job)
                    self._write_terminal_audit(job)
                self._active_jobs.discard(job_id)
            return

        with self._lock:
            self._active_jobs.discard(job_id)
            if job.status != "running" or job.provider_session_id in self._deleted_sessions:
                return
            if isinstance(result, (TextureArtifact, PixelAvatarArtifact)):
                try:
                    if isinstance(result, PixelAvatarArtifact):
                        self._store_pixel_artifact(job, result)
                    else:
                        self._store_artifact(job, result)
                except Exception:
                    self._delete_artifact(job)
                    job.status = "failed"
                    job.error = {
                        "code": "invalidArtifact",
                        "message": "artifact rejected",
                    }
                    job.updated_at = _timestamp()
                    self._persist(job)
                    self._write_terminal_audit(job)
                    return
            elif isinstance(result, dict) and job.step in {
                "analyzeIdentity",
                "completeAppearance",
            }:
                job.result = copy.deepcopy(result)
            elif isinstance(result, dict) and job.step == "generatePixelAvatar":
                job.result = copy.deepcopy(result)
            else:
                job.status = "failed"
                job.error = {
                    "code": "invalidArtifact",
                    "message": "provider returned invalid result",
                }
                job.updated_at = _timestamp()
                self._persist(job)
                self._ensure_terminal_audit(job)
                return
            job.status = "succeeded"
            job.updated_at = _timestamp()
            self._persist(job)
            self._write_terminal_audit(
                job,
                result
                if isinstance(result, (TextureArtifact, PixelAvatarArtifact))
                else None,
            )

    def status(self, job_id: str) -> JobState:
        _require_safe_id(job_id, "job id")
        with self._lock:
            job = self._jobs.get(job_id)
            if job is None:
                raise JobStoreError("job not found")
            return JobState(
                job_id=job.job_id,
                provider_session_id=job.provider_session_id,
                step=job.step,
                status=job.status,
                error=dict(job.error) if job.error is not None else None,
                artifact_id=job.artifact_id,
                artifact_sha256=job.artifact_sha256,
                result=copy.deepcopy(job.result),
                audit=self._read_terminal_audit(job.job_id),
            )

    def cancel(self, job_id: str) -> CancelReport:
        _require_safe_id(job_id, "job id")
        with self._lock:
            job = self._jobs.get(job_id)
            if job is None:
                raise JobStoreError("job not found")
            if job.status not in {"cancelled", "deleted"}:
                self._delete_artifact(job)
                job.result = None
                self._pending_requests.pop(job_id, None)
                job.status = "cancelled"
                job.updated_at = _timestamp()
                self._persist(job)
                self._ensure_terminal_audit(job)
            return CancelReport(job.job_id, job.status)

    def delete_session(self, session_id: str) -> CleanupReport:
        _require_safe_id(session_id, "provider session id")
        with self._lock:
            if session_id not in self._deleted_sessions:
                task_ids = {
                    job.lk888_task_id
                    for job_id in self._session_jobs.get(session_id, ())
                    if (job := self._jobs[job_id]).lk888_task_id is not None
                }
                self._persist_tombstone(session_id, task_ids)
                self._deleted_sessions.add(session_id)
            for job_id in tuple(self._session_jobs.get(session_id, ())):
                job = self._jobs[job_id]
                if not self._audit_path(job.job_id).exists():
                    job.status = "cancelled"
                    job.error = None
                    job.updated_at = _timestamp()
                    self._write_terminal_audit(job)
                self._delete_artifact(job)
                job.result = None
                self._pending_requests.pop(job_id, None)
                job.status = "deleted"
                job.error = None
                job.updated_at = _timestamp()
                self._state_path(job.job_id).unlink(missing_ok=True)
            return CleanupReport(backend_cleanup="deleted")

    def read_artifact(self, artifact_id: str) -> bytes:
        _require_safe_id(artifact_id, "artifact id")
        with self._lock:
            matching = next(
                (job for job in self._jobs.values() if job.artifact_id == artifact_id),
                None,
            )
            if matching is None or matching.artifact_path is None:
                raise JobStoreError("artifact not found")
            path = matching.artifact_path
            if not path.is_relative_to(self._artifacts_dir):
                raise JobStoreError("artifact path is invalid")
            try:
                return path.read_bytes()
            except FileNotFoundError as exc:
                raise JobStoreError("artifact not found") from exc

    def _store_artifact(self, job: _Job, artifact: TextureArtifact) -> None:
        if hashlib.sha256(artifact.png).hexdigest() != artifact.sha256:
            raise JobStoreError("runner returned an invalid artifact hash")
        if "layers" in artifact.coverage_report:
            try:
                semantic_audit = SemanticAtlasAuditV1.from_wire(artifact.coverage_report)
            except AuditContractError as exc:
                raise JobStoreError("runner returned an invalid semantic audit") from exc
            if (
                semantic_audit.canonical_atlas_sha256 != artifact.sha256
                or semantic_audit.body_module_id != artifact.body_module_id
                or semantic_audit.immutable_digest() != artifact.provider_raw_sha256
            ):
                raise JobStoreError("runner returned a conflicting semantic audit")
        self._store_png_artifact(
            job, artifact.png, artifact.sha256, artifact.provider_task_id
        )

    def _store_pixel_artifact(
        self, job: _Job, artifact: PixelAvatarArtifact
    ) -> None:
        audit = parse_pixel_avatar_audit(artifact.audit.to_wire())
        if (
            audit.session_id != job.session_id
            or audit.revision != job.revision
            or audit.attempt != job.attempt
            or job.lk888_task_id not in {None, audit.provider_task_id}
            or audit.normalized_sha256 != artifact.sha256
            or audit.width != artifact.width
            or audit.height != artifact.height
        ):
            raise JobStoreError("runner returned a conflicting pixel audit")
        self._store_png_artifact(
            job, artifact.png, artifact.sha256, audit.provider_task_id
        )

    def _store_png_artifact(
        self, job: _Job, png: bytes, sha256: str, provider_task_id: str
    ) -> None:
        if hashlib.sha256(png).hexdigest() != sha256:
            raise JobStoreError("runner returned an invalid artifact hash")
        _require_safe_id(provider_task_id, "task id")
        if job.lk888_task_id not in {None, provider_task_id}:
            raise JobStoreError("runner returned a conflicting task id")
        artifact_id = uuid4().hex
        path = self._artifacts_dir / f"{artifact_id}.png"
        job.artifact_id = artifact_id
        job.artifact_path = path
        job.artifact_sha256 = sha256
        job.lk888_task_id = provider_task_id
        self._write_bytes(path, png)

    def _record_lk888_task_id(self, job_id: str, task_id: str) -> None:
        _require_safe_id(task_id, "task id")
        with self._lock:
            job = self._jobs.get(job_id)
            if job is None:
                raise JobStoreError("job not found")
            if job.status == "deleted" or job.provider_session_id in self._deleted_sessions:
                self._persist_tombstone(job.provider_session_id, {task_id})
                return
            if job.status not in {"running", "cancelled"}:
                return
            if job.lk888_task_id not in {None, task_id}:
                raise JobStoreError("job already has a different task id")
            job.lk888_task_id = task_id
            job.updated_at = _timestamp()
            self._persist(job)

    def _delete_artifact(self, job: _Job) -> None:
        if job.artifact_path is not None:
            if not job.artifact_path.is_relative_to(self._artifacts_dir):
                raise JobStoreError("artifact path is invalid")
            job.artifact_path.unlink(missing_ok=True)
        job.artifact_id = None
        job.artifact_path = None
        job.artifact_sha256 = None

    def _persist(self, job: _Job) -> None:
        artifact = None
        if job.artifact_path is not None:
            artifact = {
                "path": str(job.artifact_path.relative_to(self.state_dir)).replace("\\", "/"),
                "sha256": job.artifact_sha256,
            }
        payload = {
            "artifact": artifact,
            "attempt": job.attempt,
            "auditContext": job.audit_context.to_state(),
            "createdAt": job.created_at,
            "error": job.error,
            "jobId": job.job_id,
            "lk888TaskId": job.lk888_task_id,
            "providerSessionId": job.provider_session_id,
            "revision": job.revision,
            "sessionId": job.session_id,
            "step": job.step,
            "status": job.status,
            "updatedAt": job.updated_at,
        }
        self._write_bytes(
            self._state_path(job.job_id),
            json.dumps(payload, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode("utf-8"),
        )

    def _state_path(self, job_id: str) -> Path:
        return self._jobs_dir / f"{job_id}.json"

    def _audit_path(self, job_id: str) -> Path:
        return self._audits_dir / f"{job_id}.json"

    def _write_terminal_audit(
        self,
        job: _Job,
        artifact: TextureArtifact | PixelAvatarArtifact | None = None,
    ) -> None:
        status = "cancelled" if job.status in {"cancelled", "deleted"} else job.status
        if status not in {"succeeded", "failed", "cancelled"}:
            raise JobStoreError("cannot write audit for non-terminal job")
        if isinstance(artifact, PixelAvatarArtifact):
            if status != "succeeded":
                raise JobStoreError("pixel artifact requires a succeeded job")
            self._write_audit_payload(job.job_id, artifact.audit.to_wire())
            return
        context = job.audit_context
        audit = AttemptAuditV1(
            session_id=job.session_id,
            revision=job.revision,
            attempt=job.attempt,
            provider_task_id=job.lk888_task_id,
            provider_model=context.provider_model,
            provider_raw_sha256=(artifact.provider_raw_sha256 or None) if artifact else None,
            canonical_sha256=(
                artifact.sha256 if artifact else job.artifact_sha256 if status == "succeeded" else None
            ),
            body_module_id=artifact.body_module_id if artifact else context.body_module_id,
            module_contract_sha256=(
                artifact.body_module_contract_sha256
                if artifact
                else context.module_contract_sha256
            ),
            source_texture_sha256=(
                artifact.source_texture_sha256 or None
                if artifact
                else context.source_texture_sha256
            ),
            source_alpha_sha256=(
                artifact.source_alpha_sha256 or None
                if artifact
                else context.source_alpha_sha256
            ),
            work_canvas_sha256=(
                artifact.work_canvas_sha256 or None
                if artifact
                else context.work_canvas_sha256
            ),
            region_map_sha256=(
                artifact.region_map_sha256 or None
                if artifact
                else context.region_map_sha256
            ),
            composer_version=(
                artifact.composer_version or None if artifact else context.composer_version
            ),
            png_encoder_version=(
                artifact.png_encoder_version or None
                if artifact
                else context.png_encoder_version
            ),
            coverage_report=(
                copy.deepcopy(artifact.coverage_report) if artifact else None
            ),
            status=status,
            error_code=job.error["code"] if status == "failed" and job.error else None,
            created_at=job.created_at,
            completed_at=job.updated_at,
        )
        self._write_audit_payload(job.job_id, audit.to_wire())

    def _write_audit_payload(self, job_id: str, wire: dict[str, object]) -> None:
        payload = json.dumps(
            wire, ensure_ascii=False, sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
        path = self._audit_path(job_id)
        if path.exists():
            if path.read_bytes() != payload:
                raise JobStoreError("terminal audit conflicts with immutable record")
            return
        self._write_bytes(path, payload)

    def _ensure_terminal_audit(self, job: _Job) -> None:
        if not self._audit_path(job.job_id).exists():
            self._write_terminal_audit(job)

    def _read_terminal_audit(self, job_id: str) -> dict[str, object] | None:
        path = self._audit_path(job_id)
        if not path.exists():
            return None
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(raw, dict) and "styleProfileId" in raw:
                return parse_pixel_avatar_audit(raw).to_wire()
            return AttemptAuditV1.from_wire(raw).to_wire()
        except (
            OSError,
            UnicodeError,
            json.JSONDecodeError,
            AuditContractError,
            PixelAuditError,
        ) as exc:
            raise JobStoreError("invalid terminal audit") from exc

    def _persist_tombstone(self, session_id: str, task_ids: set[str]) -> None:
        _require_safe_id(session_id, "provider session id")
        for task_id in task_ids:
            _require_safe_id(task_id, "task id")
        existing = self._tombstones.get(session_id)
        if existing is None:
            path = self._tombstones_dir / f"{uuid4().hex}.json"
            created_at = _timestamp()
            retained_task_ids: set[str] = set()
        else:
            path, created_at, retained_task_ids = existing
            retained_task_ids = set(retained_task_ids)
        retained_task_ids.update(task_ids)
        payload = {
            "createdAt": created_at,
            "lk888TaskIds": sorted(retained_task_ids),
            "providerSessionId": session_id,
        }
        self._write_bytes(
            path,
            json.dumps(payload, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode(
                "utf-8"
            ),
        )
        self._tombstones[session_id] = (path, created_at, retained_task_ids)

    def _load_tombstones(self) -> None:
        if not self._tombstones_dir.exists():
            return
        loaded: dict[str, tuple[Path, str, set[str]]] = {}
        for tombstone_path in sorted(self._tombstones_dir.glob("*.json")):
            try:
                _require_safe_id(tombstone_path.stem, "tombstone id")
                raw = json.loads(tombstone_path.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, json.JSONDecodeError, JobStoreError) as exc:
                raise JobStoreError("invalid deletion tombstone") from exc
            if not isinstance(raw, dict) or set(raw) not in {
                _LEGACY_TOMBSTONE_FIELDS,
                _TOMBSTONE_FIELDS,
            }:
                raise JobStoreError("invalid deletion tombstone fields")
            provider_session_id = _require_safe_id(
                raw["providerSessionId"], "provider session id"
            )
            created_at = _state_text(raw["createdAt"], "createdAt")
            raw_task_ids = raw.get("lk888TaskIds", [])
            if not isinstance(raw_task_ids, list):
                raise JobStoreError("invalid deletion tombstone task ids")
            task_ids = {
                _require_safe_id(task_id, "task id") for task_id in raw_task_ids
            }
            if len(task_ids) != len(raw_task_ids):
                raise JobStoreError("invalid deletion tombstone task ids")
            if provider_session_id in loaded:
                raise JobStoreError("conflicting deletion tombstone")
            loaded[provider_session_id] = (
                tombstone_path,
                created_at,
                task_ids,
            )
        self._tombstones = loaded
        self._deleted_sessions = set(loaded)

    def _load_state(self) -> None:
        if not self._jobs_dir.exists():
            return
        loaded_jobs: dict[str, _Job] = {}
        loaded_idempotency: dict[tuple[str, int, str, int], str] = {}
        loaded_session_jobs: dict[str, set[str]] = {}
        loaded_session_owner: dict[str, str] = {}
        artifact_ids: set[str] = set()
        discarded_job_ids: set[str] = set()
        discarded_idempotency: set[tuple[str, int, str, int]] = set()
        discarded_artifact_ids: set[str] = set()
        for state_path in sorted(self._jobs_dir.glob("*.json")):
            job = self._decode_state(state_path)
            if job.job_id in loaded_jobs or job.job_id in discarded_job_ids:
                raise JobStoreError("duplicate job state")
            key = (job.session_id, job.revision, job.step, job.attempt)
            if key in loaded_idempotency or key in discarded_idempotency:
                raise JobStoreError("conflicting idempotency state")
            owner = loaded_session_owner.get(job.provider_session_id)
            if owner is not None and owner != job.session_id:
                raise JobStoreError("conflicting session owner state")
            if job.provider_session_id in self._deleted_sessions:
                if job.artifact_id is not None:
                    if job.artifact_id in artifact_ids or job.artifact_id in discarded_artifact_ids:
                        raise JobStoreError("duplicate artifact state")
                    discarded_artifact_ids.add(job.artifact_id)
                discarded_job_ids.add(job.job_id)
                discarded_idempotency.add(key)
                loaded_session_owner[job.provider_session_id] = job.session_id
                self._discard_deleted_job(state_path, job)
                continue
            loaded_session_owner[job.provider_session_id] = job.session_id
            if job.artifact_id is not None:
                if job.artifact_id in artifact_ids or job.artifact_id in discarded_artifact_ids:
                    raise JobStoreError("duplicate artifact state")
                artifact_ids.add(job.artifact_id)
            loaded_jobs[job.job_id] = job
            loaded_idempotency[key] = job.job_id
            loaded_session_jobs.setdefault(job.provider_session_id, set()).add(job.job_id)

        self._jobs = loaded_jobs
        self._idempotency = loaded_idempotency
        self._session_jobs = loaded_session_jobs
        self._session_owner = loaded_session_owner
        for job in self._jobs.values():
            if job.status != "succeeded" and job.artifact_id is not None:
                self._delete_artifact(job)
                self._persist(job)
            if job.status == "running":
                job.status = "failed"
                job.error = {
                    "code": "temporaryUnavailable",
                    "message": "job interrupted",
                }
                job.updated_at = _timestamp()
                self._persist(job)
                self._ensure_terminal_audit(job)
            elif job.status == "succeeded" and job.step != "renderTextureAtlas":
                self._delete_artifact(job)
                job.status = "failed"
                job.error = {
                    "code": "temporaryUnavailable",
                    "message": "job result unavailable after restart",
                }
                job.updated_at = _timestamp()
                self._persist(job)
                self._ensure_terminal_audit(job)
            elif job.status in {"succeeded", "failed", "cancelled"}:
                self._ensure_terminal_audit(job)

    def _discard_deleted_job(self, state_path: Path, job: _Job) -> None:
        if job.artifact_path is not None:
            if not job.artifact_path.is_relative_to(self._artifacts_dir):
                raise JobStoreError("artifact path is invalid")
            job.artifact_path.unlink(missing_ok=True)
        state_path.unlink(missing_ok=True)

    def _decode_state(self, state_path: Path) -> _Job:
        job_id_from_name = state_path.stem
        try:
            _require_safe_id(job_id_from_name, "job id")
            raw = json.loads(state_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError, JobStoreError) as exc:
            raise JobStoreError("invalid job state") from exc
        if not isinstance(raw, dict) or set(raw) not in {
            _LEGACY_STATE_FIELDS,
            _STATE_FIELDS,
        }:
            raise JobStoreError("invalid job state fields")
        try:
            job_id = _require_safe_id(raw["jobId"], "job id")
            if job_id != job_id_from_name:
                raise JobStoreError("job state filename mismatch")
            session_id = _require_safe_id(raw["sessionId"], "session id")
            provider_session_id = _require_safe_id(
                raw["providerSessionId"], "provider session id"
            )
            revision = _state_int(raw["revision"], "revision", minimum=0)
            attempt = _state_int(raw["attempt"], "attempt", minimum=1, maximum=3)
            step = raw["step"]
            if not isinstance(step, str) or step not in _STEPS:
                raise JobStoreError("invalid job state step")
            status = raw["status"]
            if not isinstance(status, str) or status not in _STATUSES:
                raise JobStoreError("invalid job state status")
            created_at = _state_text(raw["createdAt"], "createdAt")
            updated_at = _state_text(raw["updatedAt"], "updatedAt")
            lk888_task_id = raw["lk888TaskId"]
            if lk888_task_id is not None:
                lk888_task_id = _require_safe_id(lk888_task_id, "task id")
            error = _decode_error(raw["error"])
            audit_context = (
                AuditContextV1.from_state(raw["auditContext"])
                if "auditContext" in raw
                else _fallback_audit_context(step)
            )
            artifact_id, artifact_path, artifact_sha256 = self._decode_artifact(
                raw["artifact"],
                status,
                allow_missing=(
                    provider_session_id in self._deleted_sessions
                    or status != "succeeded"
                ),
            )
        except JobStoreError:
            raise
        except (TypeError, ValueError) as exc:
            raise JobStoreError("invalid job state") from exc
        return _Job(
            job_id=job_id,
            session_id=session_id,
            revision=revision,
            step=step,
            attempt=attempt,
            provider_session_id=provider_session_id,
            status=status,
            created_at=created_at,
            updated_at=updated_at,
            audit_context=audit_context,
            error=error,
            artifact_id=artifact_id,
            artifact_path=artifact_path,
            artifact_sha256=artifact_sha256,
            lk888_task_id=lk888_task_id,
        )

    def _decode_artifact(
        self, raw: object, status: str, *, allow_missing: bool = False
    ) -> tuple[str | None, Path | None, str | None]:
        if raw is None:
            return None, None, None
        if not isinstance(raw, dict) or set(raw) != _ARTIFACT_FIELDS:
            raise JobStoreError("invalid artifact state")
        relative = raw["path"]
        sha256 = raw["sha256"]
        if not isinstance(relative, str) or not isinstance(sha256, str):
            raise JobStoreError("invalid artifact state")
        if (
            len(sha256) != 64
            or any(character not in "0123456789abcdef" for character in sha256)
        ):
            raise JobStoreError("invalid artifact hash")
        normalized = relative.replace("\\", "/")
        parts = normalized.split("/")
        if len(parts) != 2 or parts[0] != "artifacts" or not parts[1].endswith(".png"):
            raise JobStoreError("artifact path is invalid")
        artifact_id = parts[1][:-4]
        _require_safe_id(artifact_id, "artifact id")
        path = (self.state_dir / Path(*parts)).resolve()
        if not path.is_relative_to(self._artifacts_dir):
            raise JobStoreError("artifact path is invalid")
        if not path.exists():
            if allow_missing:
                return artifact_id, path, sha256
            raise JobStoreError("artifact path is invalid")
        if not path.is_file():
            raise JobStoreError("artifact path is invalid")
        if hashlib.sha256(path.read_bytes()).hexdigest() != sha256:
            raise JobStoreError("artifact hash is invalid")
        return artifact_id, path, sha256

    @staticmethod
    def _write_bytes(path: Path, content: bytes) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(f".{uuid4().hex}.tmp")
        try:
            temporary.write_bytes(content)
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


def _invoke_runner(
    runner: Any, request: StepRequest | PixelStepRequest, report_task_id: Any
) -> object:
    run_with_task_reporter = getattr(runner, "run_with_task_reporter", None)
    if callable(run_with_task_reporter):
        return run_with_task_reporter(request, report_task_id)
    run = getattr(runner, "run", None)
    if callable(run):
        return run(request)
    if callable(runner):
        return runner(request)
    raise TypeError("runner must be callable or provide run(request)")


def _invoke_audit_context(
    runner: Any, request: StepRequest | PixelStepRequest
) -> AuditContextV1:
    build_context = getattr(runner, "audit_context", None)
    if callable(build_context):
        context = build_context(request)
        if not isinstance(context, AuditContextV1):
            raise AuditContractError("runner audit context has an invalid type")
        return AuditContextV1.from_state(context.to_state())
    return _fallback_audit_context(request.step)


def _fallback_audit_context(step: str) -> AuditContextV1:
    model = "gpt-image-2" if step in {"renderTextureAtlas", "generatePixelAvatar"} else "gpt-4o"
    return AuditContextV1(provider_model=model)


def _safe_error(exc: Exception) -> dict[str, str]:
    if isinstance(exc, ContractError):
        return {"code": "invalidInput", "message": _PIXEL_CONTRACT_MESSAGE}
    if isinstance(exc, Lk888Error):
        return {"code": exc.code, "message": "provider failed"}
    if isinstance(exc, OSError):
        # 落盘失败（写 job 文件被拒/文件句柄失效）是本地存储故障，不是 provider 故障。
        # 单独用 localStorage 错误码，避免误导用户以为是 lk888 服务不可用。
        return {"code": "localStorage", "message": "backend storage failed"}
    return {"code": "temporaryUnavailable", "message": "provider failed"}


def _safe_contract_diagnostic(exc: ContractError) -> str:
    message = str(exc).split(";", 1)[0].strip()
    if message.startswith("pixel artifact ") and len(message) <= 128:
        return message
    return "local contract validation failed"


def _decode_error(value: object) -> dict[str, str] | None:
    if value is None:
        return None
    if not isinstance(value, dict) or set(value) != _ERROR_FIELDS:
        raise JobStoreError("invalid job error state")
    code = _require_safe_id(value["code"], "error code")
    message = _state_text(value["message"], "error message")
    return {"code": code, "message": message}


def _state_int(value: object, label: str, *, minimum: int, maximum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise JobStoreError(f"invalid {label}")
    if maximum is not None and value > maximum:
        raise JobStoreError(f"invalid {label}")
    return value


def _state_text(value: object, label: str) -> str:
    if not isinstance(value, str) or not value or len(value) > 256:
        raise JobStoreError(f"invalid {label}")
    return value


def _require_safe_id(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value) > 128
        or value in {".", ".."}
        or "/" in value
        or "\\" in value
    ):
        raise JobStoreError(f"invalid {label}")
    return value


def _timestamp() -> str:
    return datetime.now(UTC).isoformat(timespec="seconds")

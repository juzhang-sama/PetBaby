# -*- coding: utf-8 -*-
import asyncio
from pathlib import Path

import pytest

from src.provider import GenerationError, GenerationResult, TaskState
from src.storage import GenerationStorage
from src.worker import GenerationWorker


class FakeProvider:
    def __init__(self, image_bytes: bytes | None = b"png-bytes", error: str | None = None):
        self.image_bytes = image_bytes
        self.error = error
        self.calls = 0
        self.last_prompt = ""

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
        self.calls += 1
        self.last_prompt = prompt
        if on_progress is not None:
            on_progress("task-9", TaskState(task_id="task-9", state="running", is_final=False))
        if self.error:
            return GenerationResult(task_id="t1", error=self.error)
        return GenerationResult(task_id="t1", image_bytes=self.image_bytes)


class RaisingProvider:
    def generate(self, *args, **kwargs):
        raise GenerationError("network", "boom")


@pytest.fixture
def storage(tmp_path: Path) -> GenerationStorage:
    store = GenerationStorage(tmp_path / "test.db")
    store.initialize()
    return store


def make_worker(storage, provider, tmp_path: Path) -> GenerationWorker:
    return GenerationWorker(
        storage,
        lambda: provider,
        tmp_path / "data",
        poll_interval=0.01,
        max_wait=5.0,
    )


def test_process_available_completes_job(tmp_path: Path, storage: GenerationStorage) -> None:
    storage.create_job("job-1", "cat", prompt="guided prompt")
    photo_path = tmp_path / "photo.png"
    photo_path.write_bytes(b"photo-bytes")
    storage.save_source_photo(
        "p1", "job-1", "a.png", str(photo_path), "ab" * 32, 11, "image/png"
    )
    provider = FakeProvider()
    worker = make_worker(storage, provider, tmp_path)

    processed = asyncio.run(worker.process_available())

    assert processed == 1
    job = storage.get_job("job-1")
    assert job is not None
    assert job["status"] == "completed"
    assert Path(job["result_path"]).read_bytes() == b"png-bytes"
    assert provider.calls == 1
    assert provider.last_prompt == "guided prompt"


def test_process_available_marks_failed_when_result_has_error(
    tmp_path: Path, storage: GenerationStorage
) -> None:
    storage.create_job("job-1", "dog")
    provider = FakeProvider(error="task ended with state=failed")
    worker = make_worker(storage, provider, tmp_path)

    asyncio.run(worker.process_available())

    job = storage.get_job("job-1")
    assert job is not None
    assert job["status"] == "failed"
    assert job["error"] == "task ended with state=failed"


def test_process_available_marks_failed_on_provider_exception(
    tmp_path: Path, storage: GenerationStorage
) -> None:
    storage.create_job("job-1", "cat")
    worker = make_worker(storage, RaisingProvider(), tmp_path)

    asyncio.run(worker.process_available())

    job = storage.get_job("job-1")
    assert job is not None
    assert job["status"] == "failed"
    assert "network" in job["error"]


def test_process_available_returns_zero_when_queue_empty(
    tmp_path: Path, storage: GenerationStorage
) -> None:
    worker = make_worker(storage, FakeProvider(), tmp_path)
    assert asyncio.run(worker.process_available()) == 0


def test_process_available_stores_provider_task_id(
    tmp_path: Path, storage: GenerationStorage
) -> None:
    storage.create_job("job-1", "cat")
    worker = make_worker(storage, FakeProvider(), tmp_path)
    asyncio.run(worker.process_available())
    job = storage.get_job("job-1")
    assert job is not None
    assert job["provider_task_id"] == "task-9"


def test_cleanup_expired_removes_old_jobs_and_files(
    tmp_path: Path, storage: GenerationStorage
) -> None:
    storage.create_job("old", "cat")
    result_dir = tmp_path / "data" / "results" / "old"
    result_dir.mkdir(parents=True)
    result_file = result_dir / "result.png"
    result_file.write_bytes(b"x")
    storage.mark_completed("old", str(result_file))
    conn = storage._connect()
    try:
        conn.execute(
            "UPDATE generation_jobs SET updated_at = '2020-01-01T00:00:00+00:00' "
            "WHERE job_id = 'old'"
        )
    finally:
        conn.close()

    worker = make_worker(storage, FakeProvider(), tmp_path)
    removed = asyncio.run(worker.cleanup_expired(age_hours=24))

    assert removed == 1
    assert storage.get_job("old") is None
    assert not result_dir.exists()

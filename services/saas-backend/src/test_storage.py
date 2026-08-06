# -*- coding: utf-8 -*-
from pathlib import Path

import pytest

from src.storage import GenerationStorage, utc_now


@pytest.fixture
def storage(tmp_path: Path) -> GenerationStorage:
    store = GenerationStorage(tmp_path / "test.db")
    store.initialize()
    return store


def test_create_and_get_job(storage: GenerationStorage) -> None:
    storage.create_job("job-1", "cat")
    job = storage.get_job("job-1")
    assert job is not None
    assert job["job_id"] == "job-1"
    assert job["species"] == "cat"
    assert job["status"] == "queued"


def test_create_job_stores_prompt(storage: GenerationStorage) -> None:
    storage.create_job("job-1", "cat", prompt="guided prompt")
    job = storage.get_job("job-1")
    assert job is not None
    assert job["prompt"] == "guided prompt"


def test_get_missing_job_returns_none(storage: GenerationStorage) -> None:
    assert storage.get_job("nope") is None


def test_claim_next_queued_transitions_to_running(storage: GenerationStorage) -> None:
    storage.create_job("job-1", "cat")
    storage.create_job("job-2", "dog")
    first = storage.claim_next_queued()
    assert first is not None
    assert first["job_id"] == "job-1"
    assert storage.get_job("job-1")["status"] == "running"
    second = storage.claim_next_queued()
    assert second is not None
    assert second["job_id"] == "job-2"
    assert storage.claim_next_queued() is None


def test_mark_completed_and_failed(storage: GenerationStorage) -> None:
    storage.create_job("job-1", "cat")
    storage.claim_next_queued()
    storage.mark_running("job-1", "task-9")
    storage.mark_completed("job-1", "/tmp/result.png")
    job = storage.get_job("job-1")
    assert job["status"] == "completed"
    assert job["result_path"] == "/tmp/result.png"
    assert job["provider_task_id"] == "task-9"

    storage.create_job("job-2", "dog")
    storage.claim_next_queued()
    storage.mark_failed("job-2", "boom")
    job2 = storage.get_job("job-2")
    assert job2["status"] == "failed"
    assert job2["error"] == "boom"


def test_reset_stale_running_back_to_queued(storage: GenerationStorage) -> None:
    storage.create_job("job-1", "cat")
    storage.create_job("job-2", "dog")
    storage.claim_next_queued()
    storage.claim_next_queued()
    storage.mark_completed("job-2", "/tmp/x.png")
    assert storage.reset_stale_running() == 1
    assert storage.get_job("job-1")["status"] == "queued"
    assert storage.get_job("job-2")["status"] == "completed"


def test_save_and_list_photos(storage: GenerationStorage) -> None:
    storage.create_job("job-1", "cat")
    storage.save_source_photo("p1", "job-1", "a.png", "/tmp/a.png", "ab" * 32, 10, "image/png")
    photos = storage.list_photos("job-1")
    assert len(photos) == 1
    assert photos[0]["photo_id"] == "p1"
    assert photos[0]["sha256"] == "ab" * 32
    assert photos[0]["mime"] == "image/png"


def test_delete_job_removes_photos(storage: GenerationStorage) -> None:
    storage.create_job("job-1", "cat")
    storage.save_source_photo("p1", "job-1", "a.png", "/tmp/a.png", "ab" * 32, 10, "image/png")
    storage.delete_job("job-1")
    assert storage.get_job("job-1") is None
    assert storage.list_photos("job-1") == []


def test_list_jobs_older_than_filters_by_status_and_time(
    storage: GenerationStorage,
) -> None:
    storage.create_job("old-done", "cat")
    storage.mark_completed("old-done", "/tmp/x.png")
    storage.create_job("old-failed", "dog")
    storage.claim_next_queued()
    storage.mark_failed("old-failed", "boom")
    storage.create_job("old-running", "cat")
    storage.claim_next_queued()
    storage.create_job("fresh", "cat")
    storage.mark_completed("fresh", "/tmp/y.png")

    conn = storage._connect()
    try:
        conn.execute(
            "UPDATE generation_jobs SET updated_at = '2020-01-01T00:00:00+00:00' "
            "WHERE job_id IN ('old-done', 'old-failed', 'old-running')"
        )
    finally:
        conn.close()

    old = storage.list_jobs_older_than(utc_now())
    ids = {job["job_id"] for job in old}
    assert ids == {"old-done", "old-failed"}

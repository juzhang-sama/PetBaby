# -*- coding: utf-8 -*-
"""Background worker that drains the queued generation jobs."""
import asyncio
import shutil
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

from src.provider import GenerationError
from src.prompt import build_prompt
from src.storage import GenerationStorage


class GenerationWorker:
    def __init__(
        self,
        storage: GenerationStorage,
        provider_factory,
        data_dir: Path,
        poll_interval: float = 2.0,
        max_wait: float = 300.0,
    ):
        self._storage = storage
        self._provider_factory = provider_factory
        self._data_dir = Path(data_dir)
        self._poll_interval = poll_interval
        self._max_wait = max_wait
        self._task: asyncio.Task | None = None

    async def start(self) -> None:
        if self._task is None:
            self._task = asyncio.create_task(self._run())

    async def stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            self._task = None

    async def _run(self) -> None:
        last_cleanup = 0.0
        while True:
            await self.process_available()
            now = time.monotonic()
            if now - last_cleanup >= 3600:
                await self.cleanup_expired()
                last_cleanup = now
            await asyncio.sleep(self._poll_interval)

    async def process_available(self) -> int:
        processed = 0
        while True:
            job = self._storage.claim_next_queued()
            if job is None:
                return processed
            await self._process_job(job)
            processed += 1

    async def _process_job(self, job: dict) -> None:
        job_id = job["job_id"]
        provider = self._provider_factory()
        prompt = job.get("prompt") or build_prompt(job["species"])
        photos = self._storage.list_photos(job_id)
        ref_images = [
            Path(photo["stored_path"]).read_bytes()
            for photo in photos
            if Path(photo["stored_path"]).exists()
        ]
        mimes = [
            photo["mime"]
            for photo in photos
            if Path(photo["stored_path"]).exists()
        ]
        mime = mimes[0] if mimes else "image/png"

        def on_progress(task_id: str, state) -> None:
            self._storage.mark_running(job_id, task_id)
            print(
                f"[worker] job {job_id} task {task_id} state {state.state}",
                flush=True,
            )

        try:
            result = await asyncio.to_thread(
                provider.generate,
                prompt,
                ref_images or None,
                mime,
                mimes or None,
                "auto",
                self._poll_interval,
                self._max_wait,
                on_progress,
            )
        except GenerationError as exc:
            self._storage.mark_failed(job_id, f"{exc.kind}: {exc.detail}")
            return
        if result.error:
            self._storage.mark_failed(job_id, result.error)
            return
        result_dir = self._data_dir / "results" / job_id
        result_dir.mkdir(parents=True, exist_ok=True)
        result_path = result_dir / "result.png"
        result_path.write_bytes(result.image_bytes or b"")
        self._storage.mark_completed(job_id, str(result_path))

    async def cleanup_expired(self, age_hours: float = 24.0) -> int:
        cutoff = (
            datetime.now(timezone.utc) - timedelta(hours=age_hours)
        ).isoformat(timespec="seconds")
        jobs = self._storage.list_jobs_older_than(cutoff)
        for job in jobs:
            job_id = job["job_id"]
            self._storage.delete_job(job_id)
            for folder in ("photos", "results"):
                shutil.rmtree(self._data_dir / folder / job_id, ignore_errors=True)
        return len(jobs)

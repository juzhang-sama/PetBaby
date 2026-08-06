# -*- coding: utf-8 -*-
"""SQLite storage for generation jobs and source photos."""
import sqlite3
from datetime import datetime, timezone
from pathlib import Path


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


_SCHEMA = """
CREATE TABLE IF NOT EXISTS generation_jobs (
    job_id TEXT PRIMARY KEY,
    species TEXT NOT NULL,
    prompt TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    provider_task_id TEXT,
    error TEXT,
    result_path TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_photos (
    photo_id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    original_name TEXT NOT NULL,
    stored_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    mime TEXT NOT NULL,
    created_at TEXT NOT NULL
);
"""


class GenerationStorage:
    def __init__(self, db_path: Path):
        self.db_path = db_path

    def _connect(self) -> sqlite3.Connection:
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        conn = sqlite3.connect(self.db_path, isolation_level=None)
        conn.row_factory = sqlite3.Row
        return conn

    def initialize(self) -> None:
        with self._connect() as conn:
            conn.executescript(_SCHEMA)
            columns = [
                row[1]
                for row in conn.execute("PRAGMA table_info(generation_jobs)").fetchall()
            ]
            if "prompt" not in columns:
                conn.execute(
                    "ALTER TABLE generation_jobs ADD COLUMN prompt TEXT NOT NULL DEFAULT ''"
                )

    def create_job(self, job_id: str, species: str, prompt: str = "") -> None:
        now = utc_now()
        with self._connect() as conn:
            conn.execute(
                "INSERT INTO generation_jobs "
                "(job_id, species, prompt, status, created_at, updated_at) "
                "VALUES (?, ?, ?, 'queued', ?, ?)",
                (job_id, species, prompt, now, now),
            )

    def get_job(self, job_id: str) -> dict | None:
        with self._connect() as conn:
            row = conn.execute(
                "SELECT * FROM generation_jobs WHERE job_id = ?", (job_id,)
            ).fetchone()
        return dict(row) if row else None

    def claim_next_queued(self) -> dict | None:
        conn = self._connect()
        try:
            conn.execute("BEGIN IMMEDIATE")
            row = conn.execute(
                "SELECT * FROM generation_jobs "
                "WHERE status = 'queued' ORDER BY created_at LIMIT 1"
            ).fetchone()
            if row is None:
                conn.execute("COMMIT")
                return None
            conn.execute(
                "UPDATE generation_jobs SET status = 'running', updated_at = ? "
                "WHERE job_id = ?",
                (utc_now(), row["job_id"]),
            )
            conn.execute("COMMIT")
            return dict(row)
        except Exception:
            conn.execute("ROLLBACK")
            raise
        finally:
            conn.close()

    def mark_running(self, job_id: str, provider_task_id: str | None = None) -> None:
        with self._connect() as conn:
            conn.execute(
                "UPDATE generation_jobs SET status = 'running', provider_task_id = ?, "
                "updated_at = ? WHERE job_id = ?",
                (provider_task_id, utc_now(), job_id),
            )

    def mark_completed(self, job_id: str, result_path: str) -> None:
        with self._connect() as conn:
            conn.execute(
                "UPDATE generation_jobs SET status = 'completed', result_path = ?, "
                "updated_at = ? WHERE job_id = ?",
                (result_path, utc_now(), job_id),
            )

    def mark_failed(self, job_id: str, error: str) -> None:
        with self._connect() as conn:
            conn.execute(
                "UPDATE generation_jobs SET status = 'failed', error = ?, "
                "updated_at = ? WHERE job_id = ?",
                (error, utc_now(), job_id),
            )

    def reset_stale_running(self) -> int:
        with self._connect() as conn:
            cursor = conn.execute(
                "UPDATE generation_jobs SET status = 'queued', updated_at = ? "
                "WHERE status = 'running'",
                (utc_now(),),
            )
            return cursor.rowcount

    def save_source_photo(
        self,
        photo_id: str,
        job_id: str,
        original_name: str,
        stored_path: str,
        sha256: str,
        size: int,
        mime: str,
    ) -> None:
        with self._connect() as conn:
            conn.execute(
                "INSERT INTO source_photos "
                "(photo_id, job_id, original_name, stored_path, sha256, size, mime, created_at) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    photo_id,
                    job_id,
                    original_name,
                    stored_path,
                    sha256,
                    size,
                    mime,
                    utc_now(),
                ),
            )

    def list_photos(self, job_id: str) -> list[dict]:
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM source_photos WHERE job_id = ?", (job_id,)
            ).fetchall()
        return [dict(row) for row in rows]

    def list_jobs_older_than(self, older_than: str) -> list[dict]:
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM generation_jobs "
                "WHERE status IN ('completed', 'failed') AND updated_at < ?",
                (older_than,),
            ).fetchall()
        return [dict(row) for row in rows]

    def delete_job(self, job_id: str) -> None:
        with self._connect() as conn:
            conn.execute("DELETE FROM source_photos WHERE job_id = ?", (job_id,))
            conn.execute("DELETE FROM generation_jobs WHERE job_id = ?", (job_id,))

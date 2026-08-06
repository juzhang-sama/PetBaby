# -*- coding: utf-8 -*-
"""Environment-based configuration. Never hard-code secrets."""
import os
from pathlib import Path

import dotenv


def _load_env() -> None:
    env_file = Path(__file__).resolve().parent.parent / ".env"
    if env_file.exists():
        dotenv.load_dotenv(env_file)


_load_env()


def data_dir() -> Path:
    return Path(os.environ.get("SAAS_DATA_DIR", "data")).resolve()


def database_path() -> Path:
    return data_dir() / "saas-backend.db"


def lk888_api_key() -> str:
    key = os.environ.get("LK888_API_KEY", "").strip()
    if not key:
        raise RuntimeError(
            "LK888_API_KEY is not set. Copy .env.example to .env and fill it in."
        )
    return key


def lk888_base_url() -> str:
    return os.environ.get("LK888_BASE_URL", "https://api.lk888.ai").rstrip("/")


def lk888_model() -> str:
    return os.environ.get("LK888_MODEL", "gpt-image-2")


def analyze_model() -> str:
    """Vision model used to build the pet identity profile from photos.

    Empty means auto-analysis is disabled and generation falls back to
    reference-image-only prompts.
    """
    return os.environ.get("LK888_ANALYZE_MODEL", "").strip()


def poll_interval() -> float:
    return float(os.environ.get("POLL_INTERVAL", "2.0"))


def max_job_wait_seconds() -> float:
    return float(os.environ.get("MAX_JOB_WAIT_SECONDS", "300.0"))


def access_token() -> str:
    return os.environ.get("SAAS_ACCESS_TOKEN", "").strip()


def rate_limit_per_minute() -> int:
    return int(os.environ.get("RATE_LIMIT_PER_MINUTE", "10"))


def host() -> str:
    return os.environ.get("HOST", "127.0.0.1")


def port() -> int:
    return int(os.environ.get("PORT", "8787"))

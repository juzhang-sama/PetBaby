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


def api_key() -> str:
    key = os.environ.get("LK888_API_KEY", "").strip()
    if not key:
        raise RuntimeError(
            "LK888_API_KEY is not set. Copy .env.example to .env and fill it in."
        )
    return key


def base_url() -> str:
    return os.environ.get("LK888_BASE_URL", "https://api.lk888.ai").rstrip("/")


def model() -> str:
    return os.environ.get("LK888_MODEL", "gpt-image-2")

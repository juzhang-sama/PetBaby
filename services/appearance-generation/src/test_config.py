# -*- coding: utf-8 -*-
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import pytest  # noqa: E402

import config  # noqa: E402


def test_missing_key_raises(monkeypatch):
    monkeypatch.delenv("LK888_API_KEY", raising=False)
    # force a fresh read path by patching the module-level cache
    with pytest.raises(RuntimeError, match="LK888_API_KEY"):
        config.api_key()


def test_base_url_default():
    import os

    assert "api.lk888.ai" in config.base_url() or "LK888_BASE_URL" in os.environ

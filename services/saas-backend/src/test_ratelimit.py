# -*- coding: utf-8 -*-
import time

from src.ratelimit import SlidingWindowRateLimiter


def test_allows_up_to_limit() -> None:
    limiter = SlidingWindowRateLimiter(limit=3, window_seconds=60)
    assert limiter.allow("a") is True
    assert limiter.allow("a") is True
    assert limiter.allow("a") is True
    assert limiter.allow("a") is False


def test_allows_other_keys_independently() -> None:
    limiter = SlidingWindowRateLimiter(limit=1, window_seconds=60)
    assert limiter.allow("a") is True
    assert limiter.allow("b") is True
    assert limiter.allow("a") is False


def test_window_expires_and_allows_again() -> None:
    limiter = SlidingWindowRateLimiter(limit=1, window_seconds=0.05)
    assert limiter.allow("a") is True
    assert limiter.allow("a") is False
    time.sleep(0.07)
    assert limiter.allow("a") is True

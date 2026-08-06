# -*- coding: utf-8 -*-
import base64

import httpx
import pytest

from src.lk888 import Lk888Provider
from src.provider import GenerationError


class FakeResponse:
    def __init__(self, status_code=200, json_data=None, text="", content=b""):
        self.status_code = status_code
        self._json = json_data or {}
        self.text = text or str(json_data)
        self.content = content

    def json(self):
        return self._json


def test_submit_sends_expected_payload(monkeypatch) -> None:
    captured = {}

    def fake_post(url, headers=None, json=None, timeout=None):
        captured["url"] = url
        captured["body"] = json
        return FakeResponse(json_data={"data": {"task_id": "task-1"}})

    monkeypatch.setattr("httpx.post", fake_post)
    provider = Lk888Provider(key="k", base="https://x.test", model="gpt-image-2")
    task_id = provider.submit("a cat", ref_images=[b"\x89PNG-fake"], mime="image/png")
    assert task_id == "task-1"
    assert captured["url"] == "https://x.test/v1/media/generate"
    assert captured["body"]["model"] == "gpt-image-2"
    assert captured["body"]["prompt"] == "a cat"
    params = captured["body"]["params"]
    assert params["size"] == "auto"
    expected = "data:image/png;base64," + base64.b64encode(b"\x89PNG-fake").decode()
    assert params["images"] == [expected]


def test_submit_sends_per_image_mime(monkeypatch) -> None:
    captured = {}

    def fake_post(url, headers=None, json=None, timeout=None):
        captured["body"] = json
        return FakeResponse(json_data={"data": {"task_id": "task-1"}})

    monkeypatch.setattr("httpx.post", fake_post)
    provider = Lk888Provider(key="k", base="https://x.test", model="gpt-image-2")
    provider.submit(
        "a cat",
        ref_images=[b"\x89PNG-fake", b"\xff\xd8\xffjpeg"],
        mimes=["image/png", "image/jpeg"],
    )
    images = captured["body"]["params"]["images"]
    assert images[0].startswith("data:image/png;base64,")
    assert images[1].startswith("data:image/jpeg;base64,")


def test_submit_accepts_numeric_task_id(monkeypatch) -> None:
    monkeypatch.setattr(
        "httpx.post",
        lambda *a, **k: FakeResponse(json_data={"data": {"task_id": 93159178}}),
    )
    provider = Lk888Provider(key="k", base="https://x.test")
    assert provider.submit("prompt") == "93159178"


def test_submit_missing_task_id_raises(monkeypatch) -> None:
    monkeypatch.setattr(
        "httpx.post", lambda *a, **k: FakeResponse(json_data={"data": {}})
    )
    provider = Lk888Provider(key="k", base="https://x.test")
    with pytest.raises(GenerationError):
        provider.submit("prompt", retry_delay=0.01)


def test_poll_maps_success_state(monkeypatch) -> None:
    def fake_get(url, headers=None, params=None, timeout=None):
        assert params["task_id"] == "t1"
        return FakeResponse(
            json_data={
                "task_id": "t1",
                "state": "success",
                "is_final": True,
                "result_url": "https://x.test/out.png",
            }
        )

    monkeypatch.setattr("httpx.get", fake_get)
    provider = Lk888Provider(key="k", base="https://x.test")
    state = provider.poll("t1")
    assert state.is_final and state.state == "success"
    assert state.result_url == "https://x.test/out.png"


def test_download_returns_bytes(monkeypatch) -> None:
    monkeypatch.setattr(
        "httpx.get", lambda url, timeout=None: FakeResponse(content=b"image-bytes")
    )
    provider = Lk888Provider(key="k", base="https://x.test")
    assert provider.download("https://x.test/out.png") == b"image-bytes"


def test_download_retries_on_timeout_then_succeeds(monkeypatch) -> None:
    calls = {"n": 0}

    def fake_get(url, timeout=None):
        calls["n"] += 1
        if calls["n"] == 1:
            raise httpx.ReadTimeout("read timed out")
        return FakeResponse(content=b"image-bytes")

    monkeypatch.setattr("httpx.get", fake_get)
    provider = Lk888Provider(key="k", base="https://x.test")
    assert provider.download("https://x.test/out.png", retry_delay=0.01) == b"image-bytes"
    assert calls["n"] == 2


def test_download_fails_after_retries(monkeypatch) -> None:
    calls = {"n": 0}

    def fake_get(url, timeout=None):
        calls["n"] += 1
        raise httpx.ReadTimeout("read timed out")

    monkeypatch.setattr("httpx.get", fake_get)
    provider = Lk888Provider(key="k", base="https://x.test")
    with pytest.raises(GenerationError):
        provider.download("https://x.test/out.png", retries=2, retry_delay=0.01)
    assert calls["n"] == 2


def test_generate_round_trip(monkeypatch) -> None:
    calls = {"submit": 0, "poll": 0}

    def fake_post(*a, **k):
        calls["submit"] += 1
        return FakeResponse(json_data={"data": {"task_id": "t1"}})

    def fake_get(url, headers=None, params=None, timeout=None):
        calls["poll"] += 1
        if calls["poll"] == 1:
            return FakeResponse(
                json_data={"task_id": "t1", "state": "running", "is_final": False}
            )
        return FakeResponse(
            json_data={
                "task_id": "t1",
                "state": "success",
                "is_final": True,
                "result_url": "https://x.test/out.png",
            }
        )

    monkeypatch.setattr("httpx.post", fake_post)
    monkeypatch.setattr("httpx.get", fake_get)
    provider = Lk888Provider(key="k", base="https://x.test")
    provider.download = lambda url: b"image-bytes"  # type: ignore[method-assign]
    result = provider.generate("prompt", poll_interval=0.01, max_wait=10)
    assert result.image_bytes == b"image-bytes"
    assert calls["submit"] == 1


def test_generate_reports_progress(monkeypatch) -> None:
    states: list[tuple[str, str]] = []

    def fake_post(*a, **k):
        return FakeResponse(json_data={"data": {"task_id": "t1"}})

    polls = {"n": 0}

    def fake_get(url, headers=None, params=None, timeout=None):
        polls["n"] += 1
        if polls["n"] == 1:
            return FakeResponse(
                json_data={"task_id": "t1", "state": "running", "is_final": False}
            )
        return FakeResponse(
            json_data={
                "task_id": "t1",
                "state": "success",
                "is_final": True,
                "result_url": "https://x.test/out.png",
            }
        )

    monkeypatch.setattr("httpx.post", fake_post)
    monkeypatch.setattr("httpx.get", fake_get)
    provider = Lk888Provider(key="k", base="https://x.test")
    provider.download = lambda url: b"x"  # type: ignore[method-assign]
    result = provider.generate(
        "prompt",
        poll_interval=0.01,
        max_wait=10,
        on_progress=lambda task_id, state: states.append((task_id, state.state)),
    )
    assert result.image_bytes == b"x"
    assert states == [("t1", "running"), ("t1", "success")]

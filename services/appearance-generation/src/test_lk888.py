# -*- coding: utf-8 -*-
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import base64  # noqa: E402

import pytest  # noqa: E402

from lk888 import Lk888Provider  # noqa: E402
from provider import GenerationError, TaskState  # noqa: E402


class FakeResponse:
    def __init__(self, status_code=200, json_data=None, text=""):
        self.status_code = status_code
        self._json = json_data or {}
        self.text = text or str(json_data)

    def json(self):
        return self._json


def test_submit_sends_expected_payload(monkeypatch):
    captured = {}

    def fake_post(url, headers=None, json=None, timeout=None):
        captured["url"] = url
        captured["body"] = json
        return FakeResponse(json_data={"data": {"task_id": "task-1"}})

    monkeypatch.setattr("httpx.post", fake_post)
    provider = Lk888Provider(key="k", base="https://x.test", model="gpt-image-2")
    task_id = provider.submit("a cat", ref_images=[b"\x89PNG-fake"])
    assert task_id == "task-1"
    assert captured["url"] == "https://x.test/v1/media/generate"
    assert captured["body"]["model"] == "gpt-image-2"
    assert captured["body"]["prompt"] == "a cat"
    params = captured["body"]["params"]
    assert params["size"] == "auto"
    expected_data_url = "data:image/png;base64," + base64.b64encode(b"\x89PNG-fake").decode()
    assert params["images"] == [expected_data_url]


def test_submit_missing_task_id_raises(monkeypatch):
    monkeypatch.setattr(
        "httpx.post", lambda *a, **k: FakeResponse(json_data={"data": {}})
    )
    provider = Lk888Provider(key="k", base="https://x.test")
    with pytest.raises(GenerationError):
        provider.submit("prompt")


def test_poll_maps_success_state(monkeypatch):
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


def test_generate_round_trip(monkeypatch):
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

    def fake_download(url):
        return b"image-bytes"

    monkeypatch.setattr("httpx.post", fake_post)
    monkeypatch.setattr("httpx.get", fake_get)
    provider = Lk888Provider(key="k", base="https://x.test")
    provider.download = fake_download
    result = provider.generate("prompt", poll_interval=0.01, max_wait=10)
    assert result.image_bytes == b"image-bytes"
    assert calls["submit"] == 1

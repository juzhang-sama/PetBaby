# -*- coding: utf-8 -*-
import json as jsonlib

import httpx
import pytest

from src.analyzer import PetAnalyzer, _normalize_traits, _strip_code_fences


class FakeResponse:
    def __init__(self, status_code=200, text="", payload=None):
        self.status_code = status_code
        self.text = text
        self._payload = payload or {}

    def json(self):
        return self._payload


def _chat_payload(content: str) -> dict:
    return {"choices": [{"message": {"content": content}}]}


def test_analyze_sends_photos_and_returns_normalized_traits(monkeypatch) -> None:
    captured = {}

    def fake_post(url, headers=None, json=None, timeout=None):
        captured["url"] = url
        captured["body"] = json
        return FakeResponse(
            payload=_chat_payload(
                jsonlib.dumps(
                    {
                        "species": "cat",
                        "fur_colors": ["white", "cream"],
                        "pattern": "solid",
                        "ears": "pointed",
                        "eye_color": "green",
                        "face_notes": "round amber eyes, pink nose",
                    }
                )
            )
        )

    monkeypatch.setattr("httpx.post", fake_post)
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="gpt-4o")
    traits = analyzer.analyze([(b"\x89PNG-fake", "image/png")], "cat")

    assert traits is not None
    assert traits["fur_colors"] == ["white", "cream"]
    assert traits["eye_color"] == "green"
    assert traits["face_notes"] == "round amber eyes, pink nose"
    assert captured["url"] == "https://x.test/v1/chat/completions"
    body = captured["body"]
    assert body["model"] == "gpt-4o"
    assert body["response_format"] == {"type": "json_object"}
    assert body["messages"][0]["role"] == "system"
    user_content = body["messages"][1]["content"]
    assert user_content[0]["text"] == "Species hint: cat"
    assert user_content[1]["type"] == "image_url"
    assert user_content[1]["image_url"]["url"].startswith("data:image/png;base64,")


def test_analyze_strips_markdown_fences(monkeypatch) -> None:
    monkeypatch.setattr(
        "httpx.post",
        lambda *a, **k: FakeResponse(
            payload=_chat_payload('```json\n{"fur_colors": ["black"]}\n```')
        ),
    )
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="gpt-4o")
    traits = analyzer.analyze([(b"x", "image/png")], "dog")
    assert traits is not None
    assert traits["fur_colors"] == ["black"]
    assert traits["species"] == "dog"


def test_analyze_returns_none_when_model_disabled() -> None:
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="")
    assert analyzer.analyze([(b"x", "image/png")], "cat") is None


def test_analyze_returns_none_on_http_error(monkeypatch) -> None:
    def boom(*a, **k):
        raise httpx.ConnectError("network down")

    monkeypatch.setattr("httpx.post", boom)
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="gpt-4o")
    assert analyzer.analyze([(b"x", "image/png")], "cat") is None


def test_analyze_returns_none_on_non_200(monkeypatch) -> None:
    monkeypatch.setattr(
        "httpx.post", lambda *a, **k: FakeResponse(status_code=429, text="slow down")
    )
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="gpt-4o")
    assert analyzer.analyze([(b"x", "image/png")], "cat") is None


def test_analyze_returns_none_on_invalid_json(monkeypatch) -> None:
    monkeypatch.setattr(
        "httpx.post", lambda *a, **k: FakeResponse(payload=_chat_payload("not json"))
    )
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="gpt-4o")
    assert analyzer.analyze([(b"x", "image/png")], "cat") is None


def test_normalize_traits_handles_string_fur_colors() -> None:
    traits = _normalize_traits({"fur_colors": "white"}, "cat")
    assert traits["fur_colors"] == ["white"]


def test_strip_code_fences() -> None:
    assert _strip_code_fences('```json\n{"a": 1}\n```') == '{"a": 1}'
    assert _strip_code_fences('{"a": 1}') == '{"a": 1}'


def test_analyze_landmarks_returns_normalized_boxes(monkeypatch) -> None:
    box = {"x": 0.2, "y": 0.3, "width": 0.1, "height": 0.08}
    monkeypatch.setattr(
        "httpx.post",
        lambda *a, **k: FakeResponse(
            payload=_chat_payload(
                jsonlib.dumps(
                    {
                        "leftEye": box,
                        "rightEye": box,
                        "leftEar": box,
                        "rightEar": box,
                        "tail": box,
                    }
                )
            )
        ),
    )
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="gpt-4o")
    landmarks = analyzer.analyze_landmarks((b"x", "image/png"), "cat")
    assert landmarks is not None
    assert landmarks["leftEye"] == box
    assert landmarks["tail"]["width"] == 0.1


def test_analyze_landmarks_clamps_overhanging_box(monkeypatch) -> None:
    box = {"x": 0.75, "y": 0.85, "width": 0.2, "height": 0.2}
    monkeypatch.setattr(
        "httpx.post",
        lambda *a, **k: FakeResponse(
            payload=_chat_payload(
                jsonlib.dumps(
                    {
                        "leftEye": box,
                        "rightEye": box,
                        "leftEar": box,
                        "rightEar": box,
                        "tail": box,
                    }
                )
            )
        ),
    )
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="gpt-4o")
    landmarks = analyzer.analyze_landmarks((b"x", "image/png"), "cat")
    assert landmarks is not None
    assert landmarks["tail"]["height"] == pytest.approx(0.15)
    assert landmarks["tail"]["y"] + landmarks["tail"]["height"] <= 1.001


def test_analyze_landmarks_disabled_without_model() -> None:
    analyzer = PetAnalyzer(key="k", base="https://x.test", model="")
    assert analyzer.analyze_landmarks((b"x", "image/png"), "cat") is None

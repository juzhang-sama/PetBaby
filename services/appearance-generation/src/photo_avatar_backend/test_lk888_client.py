import base64
import json
import sys
from pathlib import Path

import httpx
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from photo_avatar_backend.config import BackendConfig  # noqa: E402
from photo_avatar_backend.lk888_client import (  # noqa: E402
    Lk888Client,
    Lk888Error,
)


PNG = b"\x89PNG\r\n\x1a\nsource"
UV_GUIDE = b"\x89PNG\r\n\x1a\nguide"
MP4 = b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2avc1"
FIRST_FRAME = b"\x89PNG\r\n\x1a\nfirst-frame"
PROFILE_SCHEMA = {
    "type": "object",
    "properties": {"species": {"type": "string"}},
    "required": ["species"],
    "additionalProperties": False,
}


def config() -> BackendConfig:
    return BackendConfig(
        lk888_api_key="provider-secret",
        backend_token="desktop-secret",
    )


class RecordingTransport:
    def __init__(self, *, status: int | None = None, error_code: str | None = None):
        self.status = status
        self.error_code = error_code
        self.requests: list[httpx.Request] = []
        self.json_bodies: list[dict] = []

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        if request.content:
            self.json_bodies.append(json.loads(request.content))
        if self.status is not None:
            return httpx.Response(
                self.status,
                json={
                    "error": {
                        "code": self.error_code,
                        "message": "provider rejected request",
                    }
                },
                request=request,
            )
        if request.url.path == "/v1/chat/completions":
            return httpx.Response(
                200,
                json={
                    "id": "chat-1",
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": json.dumps({"species": "cat"}),
                            },
                            "finish_reason": "stop",
                        }
                    ],
                },
                request=request,
            )
        if request.url.path == "/v1/media/generate":
            return httpx.Response(
                200,
                json={"code": 200, "msg": "created", "data": {"task_id": "task-17"}},
                request=request,
            )
        if request.url.path == "/v1/skills/task-status":
            return httpx.Response(
                200,
                json={
                    "task_id": "task-17",
                    "state": "success",
                    "is_final": True,
                    "result_url": "https://cdn.lk888.ai/result.png",
                    "error": None,
                },
                request=request,
            )
        return httpx.Response(404, request=request)


def client_with(handler) -> Lk888Client:
    return Lk888Client(config(), httpx.Client(transport=httpx.MockTransport(handler)))


def test_analysis_uses_gpt4o_and_media_uses_gpt_image_2():
    transport = RecordingTransport()
    client = client_with(transport)

    assert client.analyze_json("analyze", [PNG], PROFILE_SCHEMA) == {"species": "cat"}
    assert client.submit_image("paint atlas", [PNG, UV_GUIDE]) == "task-17"

    assert transport.requests[0].url.path == "/v1/chat/completions"
    assert transport.json_bodies[0]["model"] == "gpt-4o"
    response_format = transport.json_bodies[0]["response_format"]
    assert response_format["type"] == "json_schema"
    assert response_format["json_schema"]["name"] == "photo_avatar_analysis"
    assert response_format["json_schema"]["strict"] is True
    assert response_format["json_schema"]["schema"] == PROFILE_SCHEMA
    assert transport.requests[1].url.path == "/v1/media/generate"
    assert transport.requests[1].extensions["timeout"]["read"] == 300
    media = transport.json_bodies[1]
    assert media["model"] == "gpt-image-2"
    assert media["params"]["size"] == "2048x2048"
    assert media["params"]["quality"] == "auto"
    assert len(media["params"]["images"]) == 2


def test_media_contract_uses_numeric_task_id_and_skill_status_endpoint():
    transport = RecordingTransport()

    def handler(request: httpx.Request) -> httpx.Response:
        transport.requests.append(request)
        if request.content:
            transport.json_bodies.append(json.loads(request.content))
        if request.url.path == "/v1/media/generate":
            return httpx.Response(
                200,
                json={"code": 200, "msg": "任务创建成功", "data": {"task_id": 12345}},
                request=request,
            )
        if request.url.path == "/v1/skills/task-status":
            return httpx.Response(
                200,
                json={
                    "task_id": 12345,
                    "state": "success",
                    "is_final": True,
                    "result_url": "https://cdn.lk888.ai/result.png",
                    "error": None,
                    "status": "生成完成",
                    "progress": "100%",
                },
                request=request,
            )
        return httpx.Response(404, request=request)

    client = client_with(handler)
    assert client.submit_image("atlas", [PNG]) == "12345"
    state = client.poll_image("12345")

    assert state.task_id == "12345"
    assert state.state == "success"
    assert transport.requests[0].url.path == "/v1/media/generate"
    assert transport.requests[1].url.path == "/v1/skills/task-status"
    assert transport.requests[1].url.params["task_id"] == "12345"


def test_media_submit_rejects_non_success_envelope_code():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={"code": 403, "msg": "余额不足", "data": {}},
            request=request,
        )

    with pytest.raises(Lk888Error) as raised:
        client_with(handler).submit_image("atlas", [PNG])

    assert raised.value.code == "quota"
    assert raised.value.retryable is False


@pytest.mark.parametrize(
    "payload",
    [
        {"msg": "created", "data": {"task_id": 12345}},
        {"code": "200", "msg": "created", "data": {"task_id": 12345}},
        {"code": 200.0, "msg": "created", "data": {"task_id": 12345}},
        {"code": True, "msg": "created", "data": {"task_id": 12345}},
    ],
)
def test_media_submit_requires_exact_integer_success_code(payload):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=payload, request=request)

    with pytest.raises(Lk888Error) as raised:
        client_with(handler).submit_image("atlas", [PNG])

    assert raised.value.code == "temporaryUnavailable"
    assert raised.value.retryable is True


def test_text_only_analysis_uses_string_content_and_images_keep_multimodal_content():
    transport = RecordingTransport()
    client = client_with(transport)

    client.analyze_json("complete appearance", [], PROFILE_SCHEMA)
    client.analyze_json("analyze identity", [PNG], PROFILE_SCHEMA)

    text_only = transport.json_bodies[0]["messages"][0]["content"]
    assert text_only.startswith("complete appearance ")
    assert "valid JSON object only" in text_only
    multimodal = transport.json_bodies[1]["messages"][0]["content"]
    assert multimodal[0]["type"] == "text"
    assert multimodal[0]["text"].startswith("analyze identity ")
    assert multimodal[1]["type"] == "image_url"
    assert multimodal[1]["image_url"]["url"].startswith("data:image/png;base64,")


@pytest.mark.parametrize(
    ("status", "code", "retryable"),
    [
        (400, "invalidInput", False),
        (401, "auth", False),
        (402, "quota", False),
        (403, "auth", False),
        (429, "temporaryUnavailable", True),
        (500, "provider5xx", True),
    ],
)
def test_http_errors_are_classified(status, code, retryable):
    client = client_with(RecordingTransport(status=status))

    with pytest.raises(Lk888Error) as raised:
        client.analyze_json("analyze", [PNG], PROFILE_SCHEMA)

    assert raised.value.code == code
    assert raised.value.retryable is retryable
    assert "provider-secret" not in str(raised.value)


def test_bad_request_exposes_only_allowlisted_structured_diagnostics():
    private_values = [
        "private prompt",
        "data:image/png;base64,PRIVATE_IMAGE",
        "Authorization: Bearer private-token",
    ]
    raw_detail = (
        "invalid response_format json_schema model; " + " ".join(private_values)
    )

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            400,
            json={
                "error": {
                    "code": "bad_request",
                    "type": "invalid_request_error",
                    "param": "response_format.json_schema",
                    "message": raw_detail,
                }
            },
            request=request,
        )

    with pytest.raises(Lk888Error) as raised:
        client_with(handler).analyze_json("private prompt", [PNG], PROFILE_SCHEMA)

    assert raised.value.code == "invalidInput"
    assert raised.value.retryable is False
    assert str(raised.value) == "provider rejected request"
    assert raised.value.diagnostic == (
        "code=bad_request;type=invalid_request_error;"
        "param=response_format.json_schema;"
        "tags=response_format,json_schema,model,invalid"
    )
    for private_value in private_values:
        assert private_value not in str(raised.value)
        assert private_value not in raised.value.diagnostic


@pytest.mark.parametrize(
    ("provider_code", "expected"),
    [
        ("content_policy_violation", "contentPolicy"),
        ("insufficient_quota", "quota"),
        ("unsupported_model", "unsupported"),
    ],
)
def test_structured_provider_errors_use_existing_nonretryable_categories(
    provider_code, expected
):
    client = client_with(RecordingTransport(status=400, error_code=provider_code))

    with pytest.raises(Lk888Error) as raised:
        client.analyze_json("analyze", [PNG], PROFILE_SCHEMA)

    assert raised.value.code == expected
    assert raised.value.retryable is False


@pytest.mark.parametrize(
    ("exception", "code"),
    [
        (httpx.ConnectError("offline"), "network"),
        (httpx.ReadTimeout("slow"), "timeout"),
    ],
)
def test_transport_errors_use_existing_taxonomy(exception, code):
    def fail(request: httpx.Request) -> httpx.Response:
        exception.request = request
        raise exception

    with pytest.raises(Lk888Error) as raised:
        client_with(fail).analyze_json("analyze", [PNG], PROFILE_SCHEMA)

    assert raised.value.code == code
    assert raised.value.retryable is True


@pytest.mark.parametrize(
    "wire",
    [
        {"choices": []},
        {"choices": [{"message": {"content": "not-json"}}]},
        {"choices": [{"message": {"content": "[]"}}]},
    ],
)
def test_analysis_rejects_malformed_json_protocol(wire):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=wire, request=request)

    with pytest.raises(Lk888Error, match="analysis response") as raised:
        client_with(handler).analyze_json("analyze", [PNG], PROFILE_SCHEMA)

    assert raised.value.code == "temporaryUnavailable"
    assert raised.value.retryable is True


def test_submit_rejects_missing_task_id():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200, json={"code": 200, "msg": "created", "data": {}}, request=request
        )

    with pytest.raises(Lk888Error, match="task_id") as raised:
        client_with(handler).submit_image("atlas", [PNG])

    assert raised.value.diagnostic == (
        "media-submit:data=object;error=absent;task_id=missing;data_fields=0;"
        "top_fields=code:number,msg:string"
    )


def test_submit_missing_task_id_reports_only_known_field_shapes():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "code": 200,
                "msg": "created",
                "data": {
                    "created": 1786890000,
                    "id": "private-provider-id",
                    "images": [{"url": "https://private.example/result.png"}],
                    "private prompt": "private-user-content",
                    "status": "succeeded",
                }
            },
            request=request,
        )

    with pytest.raises(Lk888Error, match="task_id") as raised:
        client_with(handler).submit_image("atlas", [PNG])

    assert raised.value.diagnostic == (
        "media-submit:data=object;error=absent;task_id=missing;"
        "data_fields=5;"
        "known_fields=created:number,id:string,images:list,status:string;"
        "top_fields=code:number,msg:string"
    )
    assert "private-provider-id" not in raised.value.diagnostic
    assert "private-user-content" not in raised.value.diagnostic
    assert "private prompt" not in raised.value.diagnostic


def test_submit_missing_task_id_reports_safe_top_level_error_shape():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "code": 200,
                "data": {},
                "msg": "model is not supported private-user-secret",
            },
            request=request,
        )

    with pytest.raises(Lk888Error, match="task_id") as raised:
        client_with(handler).submit_image("atlas", [PNG])

    assert raised.value.diagnostic == (
        "media-submit:data=object;error=absent;task_id=missing;data_fields=0;"
        "top_fields=code:number,msg:string;top_tags=model,unsupported"
    )
    assert "private-user-secret" not in raised.value.diagnostic
    assert "model is not supported" not in raised.value.diagnostic


def test_submit_classifies_http_200_error_envelope_without_leaking_message():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "error": {
                    "code": "invalid_request_error",
                    "type": "invalid_request_error",
                    "param": "images",
                    "message": "invalid images secret-user-content",
                }
            },
            request=request,
        )

    with pytest.raises(Lk888Error) as raised:
        client_with(handler).submit_image("atlas", [PNG])

    assert raised.value.code == "invalidInput"
    assert raised.value.retryable is False
    assert raised.value.diagnostic == (
        "code=invalid_request_error;type=invalid_request_error;"
        "param=images;tags=content,invalid"
    )
    assert "secret-user-content" not in raised.value.diagnostic


def test_poll_parses_complete_media_state():
    state = client_with(RecordingTransport()).poll_image("task-17")

    assert state.task_id == "task-17"
    assert state.state == "success"
    assert state.is_final is True
    assert state.result_url == "https://cdn.lk888.ai/result.png"
    assert state.error is None


def test_poll_normalizes_empty_result_url_while_media_is_running():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "task_id": 12345,
                "state": "running",
                "is_final": False,
                "result_url": "",
                "error": None,
            },
            request=request,
        )

    state = client_with(handler).poll_image("12345")

    assert state.task_id == "12345"
    assert state.state == "running"
    assert state.is_final is False
    assert state.result_url is None
    assert state.error is None


def test_poll_rejects_empty_result_url_for_success_state():
    """success 态给出空串 result_url = 声称成功却没有产物，仍是畸形。"""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "task_id": "task-17",
                "state": "success",
                "is_final": True,
                "result_url": "",
                "error": None,
            },
            request=request,
        )

    with pytest.raises(Lk888Error):
        client_with(handler).poll_image("task-17")


@pytest.mark.parametrize(
    ("state", "raw_error", "code"),
    [
        ("failed", {"code": "temporary"}, "temporaryUnavailable"),
        ("cancelled", None, None),
    ],
)
def test_poll_accepts_empty_result_url_on_non_success_final_states(state, raw_error, code):
    """平台用**空串**表示「没有 result_url」（实测 failed 态就是 ""，不是 null）。

    空串按缺省处理；有真 URL 的 failed 仍然拒绝（见下面的反常 payload 参数表）。
    """

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "task_id": "task-17",
                "state": state,
                "is_final": True,
                "result_url": "",
                "error": raw_error,
            },
            request=request,
        )

    parsed = client_with(handler).poll_image("task-17")

    assert parsed.state == state
    assert parsed.result_url is None
    assert (parsed.error.code if parsed.error else None) == code


def test_poll_rejects_whitespace_result_url_while_media_is_running():
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "task_id": "task-17",
                "state": "running",
                "is_final": False,
                "result_url": " ",
                "error": None,
            },
            request=request,
        )

    with pytest.raises(Lk888Error):
        client_with(handler).poll_image("task-17")


@pytest.mark.parametrize(
    ("raw_error", "code", "retryable", "message"),
    [
        (
            {
                "code": "insufficient_quota",
                "message": "prompt=paint atlas data:image/png;base64,secret "
                r"C:\\Users\\Administrator\\pet.png",
            },
            "quota",
            False,
            "provider quota exhausted",
        ),
        (
            "prompt=paint atlas data:image/png;base64,secret "
            r"C:\\Users\\Administrator\\pet.png",
            "temporaryUnavailable",
            True,
            "provider media status unavailable",
        ),
    ],
)
def test_poll_maps_failed_media_error_without_echoing_untrusted_content(
    raw_error, code, retryable, message
):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "task_id": "task-17",
                "state": "failed",
                "is_final": True,
                "result_url": None,
                "error": raw_error,
            },
            request=request,
        )

    state = client_with(handler).poll_image("task-17")

    assert state.error.code == code
    assert state.error.retryable is retryable
    assert str(state.error) == message
    assert "data:image" not in str(state.error)
    assert "pet.png" not in str(state.error)


def test_poll_parses_real_content_policy_failure_payload():
    """实测 payload（seedance-2.0-guanfang-anmiao 被内容审核拒，task 135106434）。

    failed 态的 ``result_url`` 是**空串**而不是 null，``error`` 只有一句中文。
    修复前这里抛 "media status failure requires only an error"（误导性的协议错误），
    就算解开也会被 default 分支归成「可重试的临时故障」—— 重试同一份提示词必然再被拒。
    """

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "task_id": 135106434,
                "state": "failed",
                "is_final": True,
                "result_url": "",
                "result_urls": [],
                "error": "输入文本可能包含敏感信息，请调整提示词后重试。（已退款）",
                "refunded": True,
                "refunded_amount": 2.368,
                "status": "Error",
            },
            request=request,
        )

    state = client_with(handler).poll_image("135106434")

    assert state.state == "failed"
    assert state.is_final is True
    assert state.result_url is None
    assert state.error.code == "contentPolicy"
    assert state.error.retryable is False
    assert "敏感" not in str(state.error)


@pytest.mark.parametrize(
    "wire",
    [
        {
            "task_id": "task-17",
            "state": "queued",
            "is_final": False,
            "result_url": None,
            "error": {"code": "temporary", "message": "not ready"},
        },
        {
            "task_id": "task-17",
            "state": "running",
            "is_final": False,
            "result_url": None,
            "error": "untrusted running error",
        },
        {
            "task_id": "task-17",
            "state": "success",
            "is_final": True,
            "result_url": "https://cdn.lk888.ai/result.png",
            "error": {"code": "temporary", "message": "contradictory"},
        },
        {
            "task_id": "task-17",
            "state": "failed",
            "is_final": True,
            "result_url": "https://cdn.lk888.ai/result.png",
            "error": {"code": "temporary", "message": "contradictory"},
        },
        {
            "task_id": "task-17",
            "state": "cancelled",
            "is_final": True,
            "result_url": None,
            "error": {"code": "temporary", "message": "contradictory"},
        },
    ],
)
def test_poll_rejects_contradictory_media_result_and_error_combinations(wire):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=wire, request=request)

    with pytest.raises(Lk888Error, match="media status") as raised:
        client_with(handler).poll_image("task-17")

    assert raised.value.code == "temporaryUnavailable"


@pytest.mark.parametrize(
    "wire",
    [
        {"task_id": "other", "state": "running", "is_final": False},
        {"task_id": "task-17", "state": "mystery", "is_final": False},
        {"task_id": "task-17", "state": "success", "is_final": False},
    ],
)
def test_poll_rejects_mismatched_or_unknown_state(wire):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json=wire, request=request)

    with pytest.raises(Lk888Error, match="media status"):
        client_with(handler).poll_image("task-17")


class OversizedStream(httpx.SyncByteStream):
    def __iter__(self):
        yield b"\x89PNG\r\n\x1a\n"
        for _ in range(21):
            yield b"x" * (1024 * 1024)


@pytest.mark.parametrize("mode", ["redirect", "not-png", "oversized"])
def test_download_rejects_redirect_non_png_and_streamed_oversize(mode):
    def handler(request: httpx.Request) -> httpx.Response:
        if mode == "redirect":
            return httpx.Response(
                302,
                headers={"location": "https://other.example/result.png"},
                request=request,
            )
        if mode == "not-png":
            return httpx.Response(
                200,
                headers={"content-type": "text/html"},
                content=b"no",
                request=request,
            )
        return httpx.Response(
            200,
            headers={"content-type": "image/png"},
            stream=OversizedStream(),
            request=request,
        )

    with pytest.raises(Lk888Error):
        client_with(handler).download("https://cdn.lk888.ai/result.png")


def test_download_returns_png_without_following_redirects():
    seen: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return httpx.Response(
            200,
            headers={"content-type": "image/png"},
            content=PNG,
            request=request,
        )

    assert client_with(handler).download("https://cdn.lk888.ai/result.png") == PNG
    assert len(seen) == 1


def test_submit_video_uses_the_video_model_and_seedance_parameters():
    transport = RecordingTransport()
    client = client_with(transport)

    task_id = client.submit_video(
        "breathe and blink, pure green screen",
        images=[FIRST_FRAME],
        version="标准",
        duration="12",
        resolution="720p",
        aspect_ratio="1:1",
    )

    assert task_id == "task-17"
    assert transport.requests[0].url.path == "/v1/media/generate"
    assert transport.requests[0].extensions["timeout"]["read"] == 300
    body = transport.json_bodies[0]
    assert body["model"] == "seedance-2.0-guanfang-anmiao"
    assert body["params"] == {
        "version": "标准",
        "duration": "12",
        "resolution": "720p",
        "mode": "shouweizhen",
        "aspect_ratio": "1:1",
        "images": ["data:image/png;base64," + base64.b64encode(FIRST_FRAME).decode()],
    }


def test_submit_video_omits_optional_parameters_when_unset():
    transport = RecordingTransport()

    client_with(transport).submit_video(
        "a cat wagging its tail", aspect_ratio=None, mode=None
    )

    assert transport.json_bodies[0]["params"] == {
        "version": "标准",
        "duration": "5",
        "resolution": "720p",
    }


def test_submit_video_keeps_first_and_last_frames_in_order():
    transport = RecordingTransport()
    last_frame = b"\x89PNG\r\n\x1a\nlast-frame"

    client_with(transport).submit_video("settle back", images=[FIRST_FRAME, last_frame])

    encoded = transport.json_bodies[0]["params"]["images"]
    assert encoded == [
        "data:image/png;base64," + base64.b64encode(FIRST_FRAME).decode(),
        "data:image/png;base64," + base64.b64encode(last_frame).decode(),
    ]


@pytest.mark.parametrize(
    "content_type", ["video/mp4", "application/octet-stream"]
)
def test_download_video_returns_mp4_payload(content_type):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            headers={"content-type": content_type},
            content=MP4,
            request=request,
        )

    assert client_with(handler).download_video("https://cdn.lk888.ai/clip.mp4") == MP4


@pytest.mark.parametrize(
    ("content_type", "content"),
    [
        ("text/html", MP4),
        ("image/png", MP4),
        ("video/mp4", b"not-an-mp4-at-all"),
        ("video/mp4", b""),
    ],
)
def test_download_video_rejects_non_video_payloads(content_type, content):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            headers={"content-type": content_type},
            content=content,
            request=request,
        )

    with pytest.raises(Lk888Error) as raised:
        client_with(handler).download_video("https://cdn.lk888.ai/clip.mp4")

    assert raised.value.code == "invalidInput"
    assert raised.value.retryable is False


def test_download_video_still_refuses_redirects_and_plain_http():
    def redirecting(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            302,
            headers={"location": "https://other.example/clip.mp4"},
            request=request,
        )

    with pytest.raises(Lk888Error):
        client_with(redirecting).download_video("https://cdn.lk888.ai/clip.mp4")

    with pytest.raises(Lk888Error, match="HTTPS"):
        client_with(redirecting).download_video("http://cdn.lk888.ai/clip.mp4")

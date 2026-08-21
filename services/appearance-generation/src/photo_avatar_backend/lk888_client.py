"""Strict, non-retrying lk888 client for photo-avatar backend steps."""

import json
import re
from base64 import b64encode
from dataclasses import dataclass
from typing import Any, Mapping, Sequence
from urllib.parse import urlparse

import httpx

from .config import BackendConfig


_MAX_ARTIFACT_BYTES = 20 * 1024 * 1024
_MAX_PROVIDER_DIAGNOSTIC_CHARS = 300
_PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
_SAFE_PROVIDER_FIELD = re.compile(r"[A-Za-z0-9_.\[\]-]{1,80}")
_PROVIDER_DIAGNOSTIC_TAGS = (
    ("response_format", ("response_format", "response format")),
    ("json_schema", ("json_schema", "json schema")),
    ("messages", ("messages",)),
    ("content", ("content",)),
    ("required", ("required",)),
    ("additional_properties", ("additionalproperties", "additional properties")),
    ("model", ("model",)),
    ("unsupported", ("unsupported", "not supported")),
    ("invalid", ("invalid",)),
    ("missing", ("missing",)),
)
_MEDIA_STATES = frozenset(
    {"pending", "queued", "running", "success", "failed", "cancelled"}
)
_MEDIA_REQUIRED_STATE_FIELDS = frozenset(
    {"task_id", "state", "is_final", "result_url", "error"}
)


class Lk888Error(RuntimeError):
    def __init__(
        self, code: str, retryable: bool, message: str, *, diagnostic: str = ""
    ):
        super().__init__(message)
        self.code = code
        self.retryable = retryable
        self.diagnostic = diagnostic


@dataclass(frozen=True)
class MediaState:
    task_id: str
    state: str
    is_final: bool
    result_url: str | None
    error: Lk888Error | None

    @classmethod
    def parse(cls, payload: Any, expected_task_id: str) -> "MediaState":
        if not isinstance(payload, Mapping):
            raise _protocol_error("media status response must be an object")
        if not _MEDIA_REQUIRED_STATE_FIELDS <= set(payload):
            raise _protocol_error("media status fields are incomplete")
        task_id = payload.get("task_id")
        state = payload.get("state")
        is_final = payload.get("is_final")
        result_url = payload.get("result_url")
        raw_error = payload.get("error")
        normalized_task_id = _normalize_task_id(task_id)
        if normalized_task_id != expected_task_id or state not in _MEDIA_STATES:
            raise _protocol_error("media status identity or state is invalid")
        if not isinstance(is_final, bool):
            raise _protocol_error("media status final flag is invalid")
        expected_final = state in {"success", "failed", "cancelled"}
        if is_final != expected_final:
            raise _protocol_error("media status final flag contradicts state")
        if result_url is not None and not isinstance(result_url, str):
            raise _protocol_error("media status result URL is invalid")
        if result_url == "" and not is_final:
            result_url = None
        if state == "success":
            if not _is_https_url(result_url) or raw_error is not None:
                raise _protocol_error("media status success requires HTTPS result URL")
            return cls(normalized_task_id, state, is_final, result_url, None)
        if state == "failed":
            if result_url is not None or raw_error is None:
                raise _protocol_error("media status failure requires only an error")
            return cls(normalized_task_id, state, is_final, None, _media_error(raw_error))
        if result_url is not None or raw_error is not None:
            raise _protocol_error("media status non-result state cannot include result or error")
        return cls(normalized_task_id, state, is_final, None, None)


class Lk888Client:
    def __init__(self, config: BackendConfig, http: httpx.Client):
        self.config = config
        self.http = http

    def analyze_json(
        self, prompt: str, images: Sequence[bytes], schema: dict[str, Any]
    ) -> dict[str, Any]:
        prompt = (
            prompt
            + " Return one valid JSON object only, with no markdown fences or surrounding text."
        )
        content: str | list[dict[str, Any]] = prompt
        if images:
            content = [{"type": "text", "text": prompt}]
            content.extend(
                {
                    "type": "image_url",
                    "image_url": {"url": _data_url(image)},
                }
                for image in images
            )
        response = self._request(
            "POST",
            f"{self.config.lk888_base_url}/v1/chat/completions",
            timeout=60,
            json={
                "model": self.config.analysis_model,
                "messages": [{"role": "user", "content": content}],
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {
                        "name": "photo_avatar_analysis",
                        "strict": True,
                        "schema": schema,
                    },
                },
            },
        )
        wire = _response_json(response, "analysis response")
        choices = wire.get("choices") if isinstance(wire, Mapping) else None
        if not isinstance(choices, list) or len(choices) != 1:
            raise _protocol_error("analysis response must contain one choice")
        try:
            content_text = choices[0]["message"]["content"]
            result = json.loads(content_text)
        except (KeyError, TypeError, json.JSONDecodeError) as exc:
            raise _protocol_error("analysis response content is not valid JSON") from exc
        if not isinstance(result, dict):
            raise _protocol_error("analysis response JSON must be an object")
        return result

    def submit_image(self, prompt: str, images: Sequence[bytes]) -> str:
        params: dict[str, Any] = {"size": "2048x2048", "quality": "auto"}
        if images:
            params["images"] = [_data_url(image) for image in images]
        response = self._request(
            "POST",
            f"{self.config.lk888_base_url}/v1/media/generate",
            timeout=300,
            json={
                "model": self.config.image_model,
                "prompt": prompt,
                "params": params,
            },
        )
        wire = _response_json(response, "media submit response")
        if not isinstance(wire, Mapping):
            raise _protocol_error("media submit response must be an object")
        if isinstance(wire.get("error"), Mapping):
            provider_code, provider_diagnostic = _provider_error(response)
            raise _classified_provider_error(
                response.status_code, provider_code, provider_diagnostic
            )
        envelope_code = wire.get("code")
        if isinstance(envelope_code, bool) or not isinstance(envelope_code, int):
            raise _protocol_error(
                "media submit envelope code is invalid",
                diagnostic="envelope_code=" + _provider_value_shape(envelope_code),
            )
        if envelope_code != 200:
            raise _media_submit_error(wire)
        data = wire.get("data")
        task_id = data.get("task_id") if isinstance(data, Mapping) else None
        normalized_task_id = _normalize_task_id(task_id)
        if normalized_task_id is None:
            raise _protocol_error(
                "media submit response is missing task_id",
                diagnostic=_media_submit_shape(wire),
            )
        return normalized_task_id

    def poll_image(self, task_id: str) -> MediaState:
        if not task_id.strip():
            raise Lk888Error("invalidInput", False, "task_id must be non-empty")
        response = self._request(
            "GET",
            f"{self.config.lk888_base_url}/v1/skills/task-status",
            timeout=30,
            params={"task_id": task_id},
        )
        return MediaState.parse(_response_json(response, "media status response"), task_id)

    def download(self, url: str) -> bytes:
        if not _is_https_url(url):
            raise Lk888Error("invalidInput", False, "artifact URL must use HTTPS")
        try:
            with self.http.stream(
                "GET", url, follow_redirects=False, timeout=120
            ) as response:
                if response.is_redirect:
                    raise Lk888Error("invalidInput", False, "artifact redirect is forbidden")
                self._require_success(response)
                media_type = response.headers.get("content-type", "").split(";", 1)[0]
                if media_type.lower() != "image/png":
                    raise Lk888Error("invalidInput", False, "artifact is not PNG")
                declared_size = response.headers.get("content-length")
                if declared_size is not None:
                    try:
                        if int(declared_size) > _MAX_ARTIFACT_BYTES:
                            raise Lk888Error(
                                "invalidInput", False, "artifact exceeds 20 MiB"
                            )
                    except ValueError as exc:
                        raise _protocol_error("artifact content length is invalid") from exc
                body = bytearray()
                for chunk in response.iter_bytes():
                    body.extend(chunk)
                    if len(body) > _MAX_ARTIFACT_BYTES:
                        raise Lk888Error("invalidInput", False, "artifact exceeds 20 MiB")
        except Lk888Error:
            raise
        except httpx.TimeoutException as exc:
            raise Lk888Error("timeout", True, "artifact download timed out") from exc
        except httpx.HTTPError as exc:
            raise Lk888Error("network", True, "artifact download failed") from exc
        result = bytes(body)
        if not result.startswith(_PNG_SIGNATURE):
            raise Lk888Error("invalidInput", False, "artifact is not PNG")
        return result

    def _request(self, method: str, url: str, **kwargs: Any) -> httpx.Response:
        try:
            response = self.http.request(
                method,
                url,
                headers=self._headers(),
                follow_redirects=False,
                **kwargs,
            )
        except httpx.TimeoutException as exc:
            raise Lk888Error("timeout", True, "provider request timed out") from exc
        except httpx.HTTPError as exc:
            raise Lk888Error("network", True, "provider request failed") from exc
        self._require_success(response)
        return response

    def _headers(self) -> dict[str, str]:
        return {
            "Authorization": f"Bearer {self.config.lk888_api_key}",
            "Content-Type": "application/json",
        }

    @staticmethod
    def _require_success(response: httpx.Response) -> None:
        if response.is_success:
            return
        provider_code, provider_diagnostic = _provider_error(response)
        diagnostic = provider_diagnostic if response.status_code == 400 else ""
        raise _classified_provider_error(
            response.status_code, provider_code, diagnostic
        )


def _data_url(image: bytes) -> str:
    return "data:image/png;base64," + b64encode(image).decode("ascii")


def _response_json(response: httpx.Response, label: str) -> Any:
    try:
        return response.json()
    except ValueError as exc:
        raise _protocol_error(f"{label} is not valid JSON") from exc


def _provider_error(response: httpx.Response) -> tuple[str, str]:
    try:
        payload = response.json()
    except ValueError:
        return "", ""
    if not isinstance(payload, Mapping):
        return "", ""
    error = payload.get("error")
    if not isinstance(error, Mapping):
        return "", ""
    code = error.get("code")
    message = error.get("message")
    provider_code = code.lower() if isinstance(code, str) else ""
    fields = []
    for label in ("code", "type", "param"):
        safe_value = _safe_provider_field(error.get(label))
        if safe_value:
            fields.append(f"{label}={safe_value}")
    if isinstance(message, str):
        normalized_message = message.casefold()
        tags = [
            tag
            for tag, aliases in _PROVIDER_DIAGNOSTIC_TAGS
            if any(alias in normalized_message for alias in aliases)
        ]
        if tags:
            fields.append("tags=" + ",".join(tags))
    return provider_code, ";".join(fields)[:_MAX_PROVIDER_DIAGNOSTIC_CHARS]


def _safe_provider_field(value: Any) -> str:
    if not isinstance(value, str):
        return ""
    normalized = value.strip()
    return normalized if _SAFE_PROVIDER_FIELD.fullmatch(normalized) else ""


def _classified_provider_error(
    status: int, provider_code: str, diagnostic: str
) -> Lk888Error:
    if "content" in provider_code and "policy" in provider_code:
        return Lk888Error(
            "contentPolicy",
            False,
            "provider content policy rejected request",
            diagnostic=diagnostic,
        )
    if "quota" in provider_code or "credit" in provider_code:
        return Lk888Error(
            "quota", False, "provider quota exhausted", diagnostic=diagnostic
        )
    if "unsupported" in provider_code or provider_code == "model_not_found":
        return Lk888Error(
            "unsupported",
            False,
            "provider does not support request",
            diagnostic=diagnostic,
        )
    if status in {401, 403}:
        return Lk888Error("auth", False, "provider authentication failed")
    if status == 402:
        return Lk888Error("quota", False, "provider quota exhausted")
    if status == 408:
        return Lk888Error("timeout", True, "provider request timed out")
    if status == 429:
        return Lk888Error("temporaryUnavailable", True, "provider rate limited")
    if status >= 500:
        return Lk888Error("provider5xx", True, "provider unavailable")
    return Lk888Error(
        "invalidInput", False, "provider rejected request", diagnostic=diagnostic
    )


def _media_submit_shape(wire: Any) -> str:
    data = wire.get("data") if isinstance(wire, Mapping) else None
    error = wire.get("error") if isinstance(wire, Mapping) else None
    data_shape = "object" if isinstance(data, Mapping) else "absent"
    error_shape = "object" if isinstance(error, Mapping) else "absent"
    task_id = data.get("task_id") if isinstance(data, Mapping) else None
    task_shape = "string" if isinstance(task_id, str) else "missing"
    diagnostic = (
        f"media-submit:data={data_shape};error={error_shape};task_id={task_shape}"
    )
    if isinstance(data, Mapping):
        diagnostic += f";data_fields={len(data)}"
        known_fields = []
        for name in sorted(
            {
                "created",
                "data",
                "id",
                "images",
                "jobId",
                "job_id",
                "output",
                "requestId",
                "request_id",
                "resultUrl",
                "result_url",
                "state",
                "status",
                "task_id",
                "taskId",
                "url",
            }
        ):
            if name not in data:
                continue
            known_fields.append(f"{name}:{_provider_value_shape(data[name])}")
        if known_fields:
            diagnostic += ";known_fields=" + ",".join(known_fields)
    if isinstance(wire, Mapping):
        top_fields = [
            f"{name}:{_provider_value_shape(wire[name])}"
            for name in ("code", "message", "msg", "status", "success")
            if name in wire
        ]
        if top_fields:
            diagnostic += ";top_fields=" + ",".join(top_fields)
        top_message = wire.get("message", wire.get("msg"))
        if isinstance(top_message, str):
            normalized_message = top_message.casefold()
            tags = [
                tag
                for tag, aliases in _PROVIDER_DIAGNOSTIC_TAGS
                if any(alias in normalized_message for alias in aliases)
            ]
            if tags:
                diagnostic += ";top_tags=" + ",".join(tags)
    return diagnostic


def _normalize_task_id(value: Any) -> str | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int) and value > 0:
        return str(value)
    if isinstance(value, str) and value.strip():
        return value.strip()
    return None


def _media_submit_error(wire: Mapping[str, Any]) -> Lk888Error:
    raw_code = wire.get("code")
    message = wire.get("msg")
    diagnostic = "envelope_code=" + _provider_value_shape(raw_code)
    if isinstance(message, str):
        diagnostic += ";message_tags=" + ",".join(
            tag
            for tag, aliases in _PROVIDER_DIAGNOSTIC_TAGS
            if any(alias in message.casefold() for alias in aliases)
        )
    try:
        code = int(raw_code)
    except (TypeError, ValueError):
        return _protocol_error("media submit envelope code is invalid", diagnostic=diagnostic)
    if code in {402, 403}:
        return Lk888Error("quota", False, "provider quota exhausted", diagnostic=diagnostic)
    if code == 429:
        return Lk888Error("temporaryUnavailable", True, "provider rate limited", diagnostic=diagnostic)
    if code >= 500:
        return Lk888Error("provider5xx", True, "provider unavailable", diagnostic=diagnostic)
    return Lk888Error("invalidInput", False, "provider rejected request", diagnostic=diagnostic)


def _provider_value_shape(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, str):
        return "string"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, Mapping):
        return "object"
    if isinstance(value, list):
        return "list"
    return "other"


def _protocol_error(message: str, *, diagnostic: str = "") -> Lk888Error:
    return Lk888Error("temporaryUnavailable", True, message, diagnostic=diagnostic)


def _is_https_url(value: Any) -> bool:
    if not isinstance(value, str):
        return False
    parsed = urlparse(value)
    return parsed.scheme == "https" and bool(parsed.hostname)


def _media_error(value: Any) -> Lk888Error:
    if isinstance(value, Mapping):
        provider_code = value.get("code")
        if not isinstance(provider_code, str) or not provider_code.strip():
            raise _protocol_error("media status error payload is invalid")
    elif isinstance(value, str) and value.strip():
        provider_code = value
    else:
        raise _protocol_error("media status error payload is invalid")

    normalized = provider_code.lower()
    if "content" in normalized and "policy" in normalized:
        return Lk888Error(
            "contentPolicy", False, "provider content policy rejected request"
        )
    if "quota" in normalized or "credit" in normalized:
        return Lk888Error("quota", False, "provider quota exhausted")
    if "unsupported" in normalized or normalized == "model_not_found":
        return Lk888Error("unsupported", False, "provider does not support request")
    if any(token in normalized for token in ("auth", "unauthor", "forbidden")):
        return Lk888Error("auth", False, "provider authentication failed")
    if "timeout" in normalized:
        return Lk888Error("timeout", True, "provider request timed out")
    if any(token in normalized for token in ("network", "connection")):
        return Lk888Error("network", True, "provider network failure")
    if any(token in normalized for token in ("5xx", "server", "internal")):
        return Lk888Error("provider5xx", True, "provider unavailable")
    if any(token in normalized for token in ("temporary", "unavailable", "rate")):
        return Lk888Error(
            "temporaryUnavailable", True, "provider media status unavailable"
        )
    if any(token in normalized for token in ("invalid", "input", "bad_request")):
        return Lk888Error("invalidInput", False, "provider rejected request")
    return Lk888Error(
        "temporaryUnavailable", True, "provider media status unavailable"
    )

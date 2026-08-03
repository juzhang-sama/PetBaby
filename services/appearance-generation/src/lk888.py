# -*- coding: utf-8 -*-
"""Aggregation platform (lk888.ai) implementation of the generation provider."""
import base64
import time

import httpx

import config
from provider import GenerationError, GenerationResult, TaskState


class Lk888Provider:
    def __init__(self, key: str | None = None, base: str | None = None, model: str | None = None):
        self._key = key or config.api_key()
        self._base = (base or config.base_url()).rstrip("/")
        self._model = model or config.model()

    def _headers(self) -> dict[str, str]:
        return {
            "Authorization": f"Bearer {self._key}",
            "Content-Type": "application/json",
        }

    def _to_data_url(self, image: bytes) -> str:
        return "data:image/png;base64," + base64.b64encode(image).decode()

    def submit(
        self,
        prompt: str,
        ref_images: list[bytes] | None = None,
        size: str = "auto",
        retries: int = 3,
        retry_delay: float = 10.0,
    ) -> str:
        params: dict = {
            "size": size,
            "quality": "auto",
            "n": 1,
            "response_format": "url",
        }
        if ref_images:
            params["images"] = [self._to_data_url(img) for img in ref_images]
        body = {
            "model": self._model,
            "prompt": prompt,
            "params": params,
        }
        last_error: GenerationError | None = None
        for attempt in range(1, retries + 1):
            try:
                response = httpx.post(
                    f"{self._base}/v1/media/generate",
                    headers=self._headers(),
                    json=body,
                    timeout=60,
                )
            except httpx.HTTPError as exc:
                last_error = GenerationError("network", f"submit request failed: {exc}")
                if attempt < retries:
                    time.sleep(retry_delay)
                continue
            if response.status_code != 200:
                last_error = GenerationError(
                    "generation",
                    f"submit returned {response.status_code}: {response.text[:300]}",
                )
                if attempt < retries:
                    time.sleep(retry_delay)
                continue
            data = response.json().get("data", {})
            task_id = data.get("task_id")
            if not task_id:
                last_error = GenerationError(
                    "generation",
                    f"no task_id in response: {response.text[:300]}",
                )
                if attempt < retries:
                    time.sleep(retry_delay)
                continue
            return str(task_id)
        raise last_error  # type: ignore[misc]

    def poll(self, task_id: str) -> TaskState:
        try:
            response = httpx.get(
                f"{self._base}/v1/media/status",
                headers=self._headers(),
                params={"task_id": task_id},
                timeout=30,
            )
        except httpx.HTTPError as exc:
            raise GenerationError("network", f"poll request failed: {exc}", task_id) from exc
        if response.status_code != 200:
            raise GenerationError(
                "generation",
                f"poll returned {response.status_code}: {response.text[:300]}",
                task_id,
            )
        data = response.json()
        return TaskState(
            task_id=task_id,
            state=str(data.get("state", "unknown")),
            is_final=bool(data.get("is_final", False)),
            result_url=data.get("result_url") or None,
            error=data.get("error") or None,
        )

    def download(self, result_url: str) -> bytes:
        try:
            response = httpx.get(result_url, timeout=120)
        except httpx.HTTPError as exc:
            raise GenerationError("network", f"download failed: {exc}") from exc
        if response.status_code != 200:
            raise GenerationError(
                "network",
                f"download returned {response.status_code}",
            )
        return response.content

    def generate(
        self,
        prompt: str,
        ref_images: list[bytes] | None = None,
        size: str = "auto",
        poll_interval: float = 5.0,
        max_wait: float = 300.0,
    ) -> GenerationResult:
        task_id = self.submit(prompt, ref_images, size)
        deadline = time.monotonic() + max_wait
        while True:
            state = self.poll(task_id)
            if state.is_final:
                if state.state == "success" and state.result_url:
                    return GenerationResult(
                        task_id=task_id,
                        image_bytes=self.download(state.result_url),
                        result_url=state.result_url,
                    )
                return GenerationResult(
                    task_id=task_id,
                    error=state.error or f"task ended with state={state.state}",
                )
            if time.monotonic() > deadline:
                raise GenerationError("timeout", f"task {task_id} not final after {max_wait}s", task_id)
            time.sleep(poll_interval)

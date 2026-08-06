# -*- coding: utf-8 -*-
"""Aggregation platform (lk888.ai) implementation of the generation provider."""
import base64
import time

import httpx

from src.provider import GenerationError, GenerationResult, TaskState


class Lk888Provider:
    def __init__(self, key: str, base: str, model: str = "gpt-image-2"):
        self._key = key
        self._base = base.rstrip("/")
        self._model = model

    def _headers(self) -> dict[str, str]:
        return {
            "Authorization": f"Bearer {self._key}",
            "Content-Type": "application/json",
        }

    def _to_data_url(self, image: bytes, mime: str) -> str:
        return f"data:{mime};base64," + base64.b64encode(image).decode()

    def submit(
        self,
        prompt: str,
        ref_images: list[bytes] | None = None,
        mime: str = "image/png",
        mimes: list[str] | None = None,
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
            image_mimes = mimes or [mime] * len(ref_images)
            params["images"] = [
                self._to_data_url(img, img_mime)
                for img, img_mime in zip(ref_images, image_mimes)
            ]
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
            if task_id is None:
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

    def download(
        self,
        result_url: str,
        retries: int = 3,
        retry_delay: float = 10.0,
        timeout: float = 300.0,
    ) -> bytes:
        # The aggregation platform's result host is occasionally slow or
        # unreachable; retry transient failures before marking the job failed.
        last_error: GenerationError | None = None
        for attempt in range(1, retries + 1):
            try:
                response = httpx.get(result_url, timeout=timeout)
            except httpx.HTTPError as exc:
                last_error = GenerationError("network", f"download failed: {exc}")
                if attempt < retries:
                    time.sleep(retry_delay)
                continue
            if response.status_code != 200:
                last_error = GenerationError(
                    "network",
                    f"download returned {response.status_code}",
                )
                if attempt < retries:
                    time.sleep(retry_delay)
                continue
            return response.content
        raise last_error  # type: ignore[misc]

    def generate(
        self,
        prompt: str,
        ref_images: list[bytes] | None = None,
        mime: str = "image/png",
        mimes: list[str] | None = None,
        size: str = "auto",
        poll_interval: float = 5.0,
        max_wait: float = 300.0,
        on_progress=None,
    ) -> GenerationResult:
        task_id = self.submit(prompt, ref_images, mime, mimes, size)
        deadline = time.monotonic() + max_wait
        while True:
            state = self.poll(task_id)
            if on_progress is not None:
                on_progress(task_id, state)
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
                raise GenerationError(
                    "timeout", f"task {task_id} not final after {max_wait}s", task_id
                )
            time.sleep(poll_interval)

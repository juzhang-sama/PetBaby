# -*- coding: utf-8 -*-
"""Provider abstraction for image generation backends."""
from collections.abc import Callable
from dataclasses import dataclass
from typing import Protocol


@dataclass
class TaskState:
    task_id: str
    state: str  # pending / running / success / failed
    is_final: bool
    result_url: str | None = None
    error: str | None = None


@dataclass
class GenerationResult:
    task_id: str
    image_bytes: bytes | None = None
    result_url: str | None = None
    error: str | None = None


@dataclass
class GenerationError(Exception):
    kind: str  # auth / rate / network / generation / timeout
    detail: str
    task_id: str | None = None


class GenerationProvider(Protocol):
    def submit(
        self,
        prompt: str,
        ref_images: list[bytes] | None = None,
        mime: str = "image/png",
        size: str = "auto",
    ) -> str: ...

    def poll(self, task_id: str) -> TaskState: ...

    def download(self, result_url: str) -> bytes: ...

    def generate(
        self,
        prompt: str,
        ref_images: list[bytes] | None = None,
        mime: str = "image/png",
        size: str = "auto",
        poll_interval: float = 5.0,
        max_wait: float = 300.0,
        on_progress: Callable[[str, TaskState], None] | None = None,
    ) -> GenerationResult: ...

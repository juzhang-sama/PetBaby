#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "httpx>=0.28,<0.29",
#     "numpy>=2.3,<2.4",
#     "pillow>=11.3,<11.4",
#     "pydantic>=2.11,<3",
#     "python-dotenv>=1.1,<2",
# ]
# ///

# ─── How to run ───
# 1. Install uv (if not installed):
#      curl -LsSf https://astral.sh/uv/install.sh | sh
# 2. Run directly from the repository root:
#      uv run scripts/验证中等简约生成标准.py
# 3. Or run with the configured service environment:
#      python scripts/验证中等简约生成标准.py
# ──────────────────

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
import json
import os
from pathlib import Path
from typing import assert_never
from uuid import uuid4

import httpx
from dotenv import load_dotenv
from PIL import Image

from 中等简约产物 import load_logical_rgba, write_grid_preview, write_overview
from 中等简约生成 import (
    OUTPUT_ROOT,
    PETS,
    AtomicLedger,
    GenerationEvent,
    GenerationInputError,
    LedgerUpdate,
    PetSpec,
    RequestContext,
    analysis_request,
    generation_request,
    prepare_source,
)

from photo_avatar_backend.config import BackendConfig
from photo_avatar_backend.contracts import PixelAppearanceProfile
from photo_avatar_backend.lk888_client import Lk888Client, Lk888Error, MediaState
from photo_avatar_backend.pixel_audit import PixelAvatarAuditV1, PixelAvatarAuditV2
from photo_avatar_backend.pixel_avatar import analyze_pixel_identity, generate_pixel_avatar
from photo_avatar_backend.pixel_style import load_pixel_style_pack


@dataclass(frozen=True, slots=True)
class ReporterContext:
    ledger: AtomicLedger
    pet: PetSpec
    input_sha256: str
    attempt: int
    prompt_path: Path


class ReportingImageClient:
    __slots__ = ("_client", "_context", "_last_state")

    def __init__(self, client: Lk888Client, context: ReporterContext) -> None:
        self._client = client
        self._context = context
        self._last_state: str | None = None

    def submit_image(self, prompt: str, images: Sequence[bytes]) -> str:
        if len(images) != 1:
            raise GenerationInputError("v2 acceptance generation must submit one user photo")
        self._context.prompt_path.write_text(prompt, encoding="utf-8")
        task_id = self._client.submit_image(prompt, images)
        self._record(
            GenerationEvent(
                stage="submitted",
                attempt=self._context.attempt,
                taskId=task_id,
                providerState="submitted",
                errorCode=None,
            )
        )
        print(
            f"SUBMITTED {self._context.pet.label} attempt={self._context.attempt} task={task_id}",
            flush=True,
        )
        return task_id

    def poll_image(self, task_id: str) -> MediaState:
        state = self._client.poll_image(task_id)
        if state.state != self._last_state:
            self._last_state = state.state
            error_code = state.error.code if state.error is not None else None
            self._record(
                GenerationEvent(
                    stage="provider-running",
                    attempt=self._context.attempt,
                    taskId=task_id,
                    providerState=state.state,
                    errorCode=error_code,
                )
            )
            print(f"STATE task={task_id} {state.state}", flush=True)
        return state

    def download(self, url: str) -> bytes:
        return self._client.download(url)

    def _record(self, event: GenerationEvent) -> None:
        self._context.ledger.record(
            LedgerUpdate(
                pet_id=self._context.pet.pet_id,
                input_sha256=self._context.input_sha256,
                event=event,
            )
        )


def run_pet(pet: PetSpec, client: Lk888Client, ledger: AtomicLedger) -> None:
    pet_root = OUTPUT_ROOT / pet.pet_id
    pet_root.mkdir(parents=True, exist_ok=True)
    source = prepare_source(pet)
    context = RequestContext(pet, source, f"task8-{pet.pet_id}-{uuid4().hex}")
    ledger.record(
        LedgerUpdate(
            pet_id=pet.pet_id,
            input_sha256=source.sha256,
            event=GenerationEvent(
                stage="analyzing", attempt=0, taskId=None, providerState=None, errorCode=None
            ),
        )
    )
    profile_raw = analyze_pixel_identity(analysis_request(context), client=client)
    profile = PixelAppearanceProfile.parse(profile_raw)
    (pet_root / "身份档案.json").write_text(
        json.dumps(profile_raw, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    ledger.record(
        LedgerUpdate(
            pet_id=pet.pet_id,
            input_sha256=source.sha256,
            event=GenerationEvent(
                stage="analyzed", attempt=0, taskId=None, providerState=None, errorCode=None
            ),
        )
    )
    style = load_pixel_style_pack()
    for attempt in range(1, 4):
        reporter = ReportingImageClient(
            client,
            ReporterContext(
                ledger, pet, source.sha256, attempt, pet_root / f"生成提示词-尝试{attempt}.txt"
            ),
        )
        try:
            artifact = generate_pixel_avatar(
                generation_request(context, profile, attempt),
                client=reporter,
                style=style,
                poll_interval_seconds=10,
                max_wait_seconds=600,
            )
        except Lk888Error as error:
            ledger.record(
                LedgerUpdate(
                    pet_id=pet.pet_id,
                    input_sha256=source.sha256,
                    event=GenerationEvent(
                        stage="failed",
                        attempt=attempt,
                        taskId=None,
                        providerState="failed",
                        errorCode=error.code,
                    ),
                )
            )
            if error.retryable and attempt < 3:
                print(f"RETRY {pet.label} code={error.code}", flush=True)
                continue
            raise
        break
    else:
        raise GenerationInputError(f"{pet.label} exhausted three generation attempts")
    match artifact.audit:
        case PixelAvatarAuditV2() as audit:
            audit_wire = audit.to_wire()
        case PixelAvatarAuditV1():
            raise GenerationInputError(f"{pet.label} unexpectedly produced audit v1")
        case unreachable:
            assert_never(unreachable)
    master_path = pet_root / "母版.png"
    master_path.write_bytes(artifact.png)
    (pet_root / "审计.json").write_text(
        json.dumps(audit_wire, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    logical = load_logical_rgba(master_path)
    Image.fromarray(logical, "RGBA").save(pet_root / "逻辑网格160.png")
    write_grid_preview(logical, pet_root / "逻辑网格160-坐标.png")
    entries = [("Photo", pet.source_path)]
    if pet.old_detail_path is not None:
        entries.append(("Old high detail", pet.old_detail_path))
    entries.append(("V2 medium simple", master_path))
    write_overview(entries, pet_root / "静态对比.png")
    ledger.record(
        LedgerUpdate(
            pet_id=pet.pet_id,
            input_sha256=source.sha256,
            event=GenerationEvent(
                stage="succeeded",
                attempt=attempt,
                taskId=audit.provider_task_id,
                providerState="success",
                errorCode=None,
            ),
        )
    )
    print(f"SAVED {pet.label} task={audit.provider_task_id} -> {master_path}", flush=True)


def main() -> int:
    OUTPUT_ROOT.mkdir(parents=True, exist_ok=True)
    ledger = AtomicLedger(OUTPUT_ROOT / "生成任务.json", PETS)
    environment_path = (
        Path(__file__).resolve().parents[1] / "services" / "appearance-generation" / ".env"
    )
    load_dotenv(environment_path)
    config = BackendConfig.from_env(os.environ)
    failures = 0
    with httpx.Client() as http:
        client = Lk888Client(config, http)
        for pet in PETS:
            try:
                run_pet(pet, client, ledger)
            except (GenerationInputError, Lk888Error, OSError) as error:
                failures += 1
                print(f"FAILED {pet.label}: {error}", flush=True)
    if failures:
        print(f"COMPLETE success={len(PETS) - failures} failed={failures}", flush=True)
        return 1
    write_overview(
        tuple((pet.label, OUTPUT_ROOT / pet.pet_id / "母版.png") for pet in PETS),
        OUTPUT_ROOT / "三宠物v2母版总览.png",
    )
    print("COMPLETE success=3 failed=0", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from enum import StrEnum
from io import BytesIO
import hashlib
import os
from pathlib import Path
import sys
from typing import Final

from PIL import Image
from pydantic import BaseModel, ConfigDict, Field

REPO_ROOT: Final = Path(__file__).resolve().parents[1]
SERVICE_SOURCE: Final = REPO_ROOT / "services" / "appearance-generation" / "src"
sys.path.insert(0, str(SERVICE_SOURCE))

from photo_avatar_backend.contracts import (  # noqa: E402
    PixelAppearanceProfile,
    PixelStepRequest,
    SourceImage,
)

STYLE_ID: Final = "pixel-style-v2-animation-ready"
OUTPUT_ROOT: Final = REPO_ROOT / "output" / "中等简约像素标准验收-2026-08-21"


@dataclass(frozen=True, slots=True)
class PetSpec:
    pet_id: str
    label: str
    source_path: Path
    old_detail_path: Path | None


PETS: Final = (
    PetSpec(
        "01-longhair-black-white",
        "长毛黑白猫",
        Path(r"C:\Users\Administrator\Desktop\ChatGPT Image 2026年8月7日 18_32_00.png"),
        REPO_ROOT / "output/宠物简约风格对比-2026-08-21/01-longhair-black-white/高细节.png",
    ),
    PetSpec(
        "02-round-tabby",
        "圆脸狸花猫",
        Path(r"C:\Users\Administrator\Desktop\毛砌墙.jpg"),
        REPO_ROOT / "output/宠物简约风格对比-2026-08-21/02-round-tabby/高细节.png",
    ),
    PetSpec(
        "03-sleek-black",
        "修长黑猫",
        Path(r"C:\Users\Administrator\Desktop\扭扭.jpg"),
        REPO_ROOT / "output/宠物简约风格对比-2026-08-21/03-sleek-black/高细节.png",
    ),
)


@dataclass(frozen=True, slots=True)
class GenerationInputError(Exception):
    message: str

    def __str__(self) -> str:
        return self.message


@dataclass(frozen=True, slots=True)
class PreparedSource:
    png: bytes
    sha256: str
    width: int
    height: int

    def as_contract(self, source_id: str) -> SourceImage:
        return SourceImage(source_id, self.png, self.sha256, self.width, self.height)


@dataclass(frozen=True, slots=True)
class RequestContext:
    pet: PetSpec
    source: PreparedSource
    session_id: str


class GenerationStage(StrEnum):
    READY = "ready"
    ANALYZING = "analyzing"
    ANALYZED = "analyzed"
    SUBMITTED = "submitted"
    PROVIDER_RUNNING = "provider-running"
    SUCCEEDED = "succeeded"
    FAILED = "failed"


class GenerationEvent(BaseModel):
    model_config = ConfigDict(frozen=True, populate_by_name=True)

    stage: GenerationStage
    attempt: int = Field(ge=0, le=3)
    task_id: str | None = Field(alias="taskId")
    provider_state: str | None = Field(alias="providerState")
    error_code: str | None = Field(alias="errorCode")
    recorded_at: str = Field(
        default_factory=lambda: datetime.now(timezone.utc).isoformat(),
        alias="recordedAt",
    )


class JobRecord(BaseModel):
    model_config = ConfigDict(frozen=True, populate_by_name=True)

    pet_id: str = Field(alias="petId")
    pet_label: str = Field(alias="petLabel")
    input_path: str = Field(alias="inputPath")
    input_sha256: str | None = Field(alias="inputSha256")
    events: tuple[GenerationEvent, ...] = ()


class LedgerDocument(BaseModel):
    model_config = ConfigDict(frozen=True, populate_by_name=True)

    schema_version: int = Field(default=1, alias="schemaVersion")
    style_profile_id: str = Field(default=STYLE_ID, alias="styleProfileId")
    jobs: tuple[JobRecord, ...]


class LedgerUpdate(BaseModel):
    model_config = ConfigDict(frozen=True)

    pet_id: str
    event: GenerationEvent
    input_sha256: str | None = None


class AtomicLedger:
    __slots__ = ("_path", "_state")

    def __init__(self, path: Path, pets: tuple[PetSpec, ...]) -> None:
        self._path = path
        self._state = LedgerDocument(
            jobs=tuple(
                JobRecord(
                    petId=pet.pet_id,
                    petLabel=pet.label,
                    inputPath=str(pet.source_path),
                    inputSha256=None,
                )
                for pet in pets
            )
        )
        self._write()

    def record(self, update: LedgerUpdate) -> None:
        matched = False
        jobs = []
        for job in self._state.jobs:
            if job.pet_id == update.pet_id:
                matched = True
                jobs.append(
                    job.model_copy(
                        update={
                            "input_sha256": update.input_sha256 or job.input_sha256,
                            "events": (*job.events, update.event),
                        }
                    )
                )
            else:
                jobs.append(job)
        if not matched:
            raise GenerationInputError(f"unknown pet ledger id: {update.pet_id}")
        self._state = self._state.model_copy(update={"jobs": tuple(jobs)})
        self._write()

    def _write(self) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True)
        temporary = self._path.with_suffix(self._path.suffix + ".tmp")
        temporary.write_text(
            self._state.model_dump_json(by_alias=True, indent=2),
            encoding="utf-8",
        )
        os.replace(temporary, self._path)


def prepare_source(pet: PetSpec) -> PreparedSource:
    if not pet.source_path.is_file():
        raise GenerationInputError(f"input photo is missing: {pet.source_path}")
    with Image.open(pet.source_path) as image:
        converted = image.convert("RGB")
        width, height = converted.size
        if not 256 <= width <= 4096 or not 256 <= height <= 4096:
            raise GenerationInputError(f"input photo dimensions are unsupported: {width}x{height}")
        if width * height > 16_000_000:
            raise GenerationInputError("input photo exceeds the production pixel limit")
        output = BytesIO()
        converted.save(output, format="PNG")
    png = output.getvalue()
    return PreparedSource(png, hashlib.sha256(png).hexdigest(), width, height)


def analysis_request(context: RequestContext) -> PixelStepRequest:
    return PixelStepRequest(
        style_profile_id=STYLE_ID,
        session_id=context.session_id,
        revision=1,
        provider_session_id=None,
        step="analyzeIdentity",
        attempt=1,
        consent_version="photo-avatar-third-party-ai-lk888-no-delete-v2",
        source_images=(context.source.as_contract(context.pet.pet_id),),
        profile=None,
        modification=None,
        locked_traits=(),
    )


def generation_request(
    context: RequestContext,
    profile: PixelAppearanceProfile,
    attempt: int,
) -> PixelStepRequest:
    return PixelStepRequest(
        style_profile_id=STYLE_ID,
        session_id=context.session_id,
        revision=1,
        provider_session_id=context.session_id,
        step="generatePixelAvatar",
        attempt=attempt,
        consent_version="photo-avatar-third-party-ai-lk888-no-delete-v2",
        source_images=(context.source.as_contract(context.pet.pet_id),),
        profile=profile,
        modification=None,
        locked_traits=(),
    )


__all__ = [
    "AtomicLedger",
    "GenerationEvent",
    "GenerationInputError",
    "GenerationStage",
    "LedgerUpdate",
    "OUTPUT_ROOT",
    "PETS",
    "PetSpec",
    "PreparedSource",
    "RequestContext",
    "STYLE_ID",
    "analysis_request",
    "generation_request",
    "prepare_source",
]

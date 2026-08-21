from __future__ import annotations

import json
from pathlib import Path
import sys

from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))

from 中等简约生成 import (  # noqa: E402
    PETS,
    STYLE_ID,
    AtomicLedger,
    GenerationEvent,
    LedgerUpdate,
    PetSpec,
    RequestContext,
    analysis_request,
    prepare_source,
)


def test_task8_uses_three_named_photos_and_v2_style() -> None:
    assert STYLE_ID == "pixel-style-v2-animation-ready"
    assert tuple(pet.label for pet in PETS) == ("长毛黑白猫", "圆脸狸花猫", "修长黑猫")
    assert tuple(pet.source_path.name for pet in PETS) == (
        "ChatGPT Image 2026年8月7日 18_32_00.png",
        "毛砌墙.jpg",
        "扭扭.jpg",
    )


def test_analysis_request_contains_only_the_user_photo(tmp_path: Path) -> None:
    source_path = tmp_path / "猫.png"
    Image.new("RGB", (256, 256), (40, 50, 60)).save(source_path)
    pet = PetSpec("test-cat", "测试猫", source_path, None)
    prepared = prepare_source(pet)

    request = analysis_request(RequestContext(pet, prepared, "session-test"))

    assert request.style_profile_id == STYLE_ID
    assert request.step == "analyzeIdentity"
    assert len(request.source_images) == 1
    assert request.source_images[0].sha256 == prepared.sha256


def test_ledger_atomically_records_real_task_state(tmp_path: Path) -> None:
    ledger_path = tmp_path / "生成任务.json"
    pet = PetSpec("test-cat", "测试猫", tmp_path / "猫.png", None)
    ledger = AtomicLedger(ledger_path, (pet,))

    ledger.record(
        LedgerUpdate(
            pet_id=pet.pet_id,
            input_sha256="a" * 64,
            event=GenerationEvent(
                stage="provider-running",
                attempt=1,
                task_id="108652999",
                provider_state="running",
                error_code=None,
            ),
        )
    )

    wire = json.loads(ledger_path.read_text(encoding="utf-8"))
    assert wire["jobs"][0]["inputSha256"] == "a" * 64
    assert wire["jobs"][0]["events"][-1]["taskId"] == "108652999"
    assert wire["jobs"][0]["events"][-1]["providerState"] == "running"
    assert not ledger_path.with_suffix(".json.tmp").exists()


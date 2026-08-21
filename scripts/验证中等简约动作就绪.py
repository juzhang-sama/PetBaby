#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy>=2.3,<2.4", "pillow>=11.3,<11.4", "pydantic>=2.11,<3"]
# ///

# ─── How to run ───
# 1. Install uv (if not installed):
#      curl -LsSf https://astral.sh/uv/install.sh | sh
# 2. Run from the repository root:
#      uv run scripts/验证中等简约动作就绪.py output/中等简约像素标准验收-2026-08-21
# 3. Or use the configured project environment:
#      python scripts/验证中等简约动作就绪.py output/中等简约像素标准验收-2026-08-21
# ──────────────────

from __future__ import annotations

from dataclasses import asdict, dataclass
import json
from pathlib import Path
import sys

from pydantic import ValidationError

from 中等简约产物 import write_overview
from 中等简约动作 import MotionAnnotation, MotionError
from 中等简约生成 import OUTPUT_ROOT, PETS, PetSpec
from 像素动作库 import ActionResult, run_action_library


@dataclass(frozen=True, slots=True)
class MotionJob:
    pet: PetSpec
    master_path: Path
    annotation_path: Path
    annotation: MotionAnnotation


@dataclass(frozen=True, slots=True)
class MotionResult:
    job: MotionJob
    actions: tuple[ActionResult, ...]
    overview_path: Path


def load_jobs(output_root: Path) -> tuple[MotionJob, ...]:
    jobs = []
    for pet in PETS:
        master_path = output_root / pet.pet_id / "母版.png"
        annotation_path = output_root / "annotations" / f"{pet.pet_id}.json"
        if not master_path.is_file():
            raise MotionError(f"missing master image: {master_path}")
        if not annotation_path.is_file():
            raise MotionError(f"missing annotation: {annotation_path}")
        annotation = MotionAnnotation.model_validate_json(
            annotation_path.read_text(encoding="utf-8")
        )
        jobs.append(MotionJob(pet, master_path, annotation_path, annotation))
    return tuple(jobs)


def run_job(job: MotionJob, output_root: Path) -> MotionResult:
    action_root = output_root / job.pet.pet_id / "动作"
    actions = run_action_library(job.master_path, job.annotation_path, action_root)
    overview_entries = [("Rest", job.master_path)]
    overview_entries.extend(
        (action.audit.action_id, action.artifact.peak_path) for action in actions
    )
    overview_path = output_root / job.pet.pet_id / "动作总览.png"
    write_overview(tuple(overview_entries), overview_path)
    return MotionResult(job, actions, overview_path)


def write_audit(results: tuple[MotionResult, ...], output_root: Path) -> Path:
    jobs = []
    for result in results:
        jobs.append(
            {
                "petId": result.job.pet.pet_id,
                "petLabel": result.job.pet.label,
                "annotation": str(result.job.annotation_path),
                "tail": {
                    "enabled": result.job.annotation.tail.enabled,
                    "disabledReason": result.job.annotation.tail.disabled_reason,
                },
                "actions": [
                    {
                        **asdict(action.audit),
                        "gif": str(action.artifact.gif_path),
                        "peak": str(action.artifact.peak_path),
                    }
                    for action in result.actions
                ],
            }
        )
    audit_path = output_root / "动作审计.json"
    audit_path.write_text(
        json.dumps({"schemaVersion": 1, "jobs": jobs}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    return audit_path


def main() -> int:
    if len(sys.argv) > 2:
        print("usage: python scripts/验证中等简约动作就绪.py [验收输出目录]")
        return 2
    output_root = Path(sys.argv[1]) if len(sys.argv) == 2 else OUTPUT_ROOT
    try:
        jobs = load_jobs(output_root)
    except (MotionError, ValidationError, OSError) as error:
        print(f"ANNOTATION FAILED: {error}", flush=True)
        return 2
    try:
        results = tuple(run_job(job, output_root) for job in jobs)
        audit_path = write_audit(results, output_root)
        overview_entries = []
        for result in results:
            overview_entries.append(
                (f"{result.job.pet.label} / Rest", result.job.master_path)
            )
            overview_entries.extend(
                (
                    f"{result.job.pet.label} / {action.audit.action_id}",
                    action.artifact.peak_path,
                )
                for action in result.actions
            )
        write_overview(
            tuple(overview_entries),
            output_root / "三宠物动作总览.png",
            columns=4,
        )
    except (MotionError, OSError) as error:
        print(f"MOTION FAILED: {error}", flush=True)
        return 1
    print(f"MOTION READY pets={len(results)} audit={audit_path}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy>=2.3,<2.4", "pillow>=11.3,<11.4", "pydantic>=2.11,<3"]
# ///

# ─── How to run ───
# 1. Install uv (if not installed):
#      curl -LsSf https://astral.sh/uv/install.sh | sh
# 2. Run from the repository root:
#      uv run scripts/像素动作库.py <母版.png> <标注.json> [输出目录]
# 3. Or use the configured project environment:
#      python scripts/像素动作库.py <母版.png> <标注.json> [输出目录]
# ──────────────────

from __future__ import annotations

from dataclasses import asdict, dataclass
import json
from pathlib import Path
import sys

import numpy as np

from 中等简约产物 import ActionArtifact, ActionExportSpec, export_action, load_logical_rgba
from 中等简约动作 import (
    ActionAuditSpec,
    MotionAnnotation,
    MotionAudit,
    Rect,
    audit_action,
    make_blink,
    make_breath,
    make_tail_wag,
)


@dataclass(frozen=True, slots=True)
class ActionDefinition:
    action_id: str
    duration_ms: int
    frames: tuple[np.ndarray, ...]
    target: Rect


@dataclass(frozen=True, slots=True)
class ActionResult:
    audit: MotionAudit
    artifact: ActionArtifact


def build_actions(
    source: np.ndarray, annotation: MotionAnnotation
) -> tuple[ActionDefinition, ...]:
    actions = [
        ActionDefinition(
            "breath", 180, make_breath(source, annotation), annotation.breath_zone
        ),
        ActionDefinition(
            "blink", 150, make_blink(source, annotation), annotation.eye_bounds
        ),
    ]
    if annotation.tail.enabled:
        actions.append(
            ActionDefinition(
                "tail-wag",
                120,
                make_tail_wag(source, annotation),
                annotation.tail.bounds,
            )
        )
    return tuple(actions)


def run_action_library(
    master_path: Path, annotation_path: Path, output_root: Path
) -> tuple[ActionResult, ...]:
    annotation = MotionAnnotation.model_validate_json(
        annotation_path.read_text(encoding="utf-8")
    )
    source = load_logical_rgba(master_path)
    results = []
    for action in build_actions(source, annotation):
        audit = audit_action(
            ActionAuditSpec(action.action_id, annotation, action.target),
            source,
            action.frames,
        )
        artifact = export_action(
            ActionExportSpec(action.action_id, output_root, action.duration_ms),
            action.frames,
        )
        results.append(ActionResult(audit, artifact))
    return tuple(results)


def main() -> int:
    if len(sys.argv) not in {3, 4}:
        print("usage: python scripts/像素动作库.py <母版.png> <标注.json> [输出目录]")
        return 2
    master_path = Path(sys.argv[1])
    annotation_path = Path(sys.argv[2])
    output_root = Path(sys.argv[3]) if len(sys.argv) == 4 else Path("output/像素动作库")
    results = run_action_library(master_path, annotation_path, output_root)
    (output_root / "动作审计.json").write_text(
        json.dumps([asdict(result.audit) for result in results], ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    for result in results:
        print(
            f"{result.audit.action_id}: {result.audit.frame_count} frames -> "
            f"{result.artifact.gif_path}",
            flush=True,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

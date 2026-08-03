# -*- coding: utf-8 -*-
"""CLI entry points for the appearance generation experiment."""
import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))

import config  # noqa: E402
from lk888 import Lk888Provider  # noqa: E402


def smoke(ref_path: Path | None) -> None:
    """Generate one image to output/smoke/ to verify the full pipeline."""
    provider = Lk888Provider()
    prompt = (
        "A cute chibi-style cat sitting upright, front view, full body, "
        "plain light grey background."
    )
    ref_images = None
    if ref_path and ref_path.exists():
        ref_images = [ref_path.read_bytes()]
        prompt = (
            "Convert this cat reference photo into a cute chibi cartoon style. "
            "Keep the same fur colors, ear shape and eye color. "
            "Front view, sitting upright, plain light grey background, full body."
        )
    print(f"submitting to {config.base_url()} model={config.model()} ref={bool(ref_images)}")
    result = provider.generate(prompt, ref_images=ref_images)
    out_dir = ROOT / "output" / "smoke"
    out_dir.mkdir(parents=True, exist_ok=True)
    if result.image_bytes:
        path = out_dir / f"smoke-{result.task_id}.png"
        path.write_bytes(result.image_bytes)
        print(f"OK -> {path} ({len(result.image_bytes)} bytes)")
    else:
        print(f"FAILED task={result.task_id} error={result.error}")
        sys.exit(1)


def main() -> None:
    parser = argparse.ArgumentParser(description="appearance generation experiment")
    sub = parser.add_subparsers(dest="command", required=True)
    smoke_parser = sub.add_parser("smoke", help="run a smoke generation")
    smoke_parser.add_argument("--ref", type=Path, default=None, help="reference image path")
    args = parser.parse_args()
    if args.command == "smoke":
        smoke(args.ref)


if __name__ == "__main__":
    main()

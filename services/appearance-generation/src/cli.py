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


def run_experiment(args) -> None:
    """Run the full candidate experiment for one photo."""
    from PIL import Image

    from evaluate import render_evaluation_html, save_evaluation_json
    from filter import filter_candidates
    from generate_batch import BatchConfig, BatchRunner, postprocess_candidate
    from identity import LockedTraits

    photo_path = Path(args.photo)
    if not photo_path.exists():
        print(f"photo not found: {photo_path}")
        sys.exit(1)

    traits = LockedTraits.from_dict(args.traits or {})
    output_dir = Path(__file__).resolve().parent / "output" / "experiments" / photo_path.stem
    provider = Lk888Provider()
    runner = BatchRunner(provider, output_dir)
    batch = BatchConfig(
        photo_path=photo_path,
        traits=traits,
        count=args.count,
        subject_desc=args.subject or "",
    )
    print(f"generating {args.count} candidates for {photo_path.name} ...")
    candidates = runner.run(batch)

    processed = []
    for candidate in candidates:
        entry = {
            "index": candidate.index,
            "task_id": candidate.task_id,
            "image": None,
            "error": candidate.error,
        }
        if candidate.image is not None:
            try:
                entry["image"] = postprocess_candidate(candidate.image)
            except Exception as exc:
                entry["error"] = f"postprocess: {exc}"
        processed.append(entry)

    report = filter_candidates(
        [entry.get("image") for entry in processed]
    )
    print(f"filter: kept={report.kept} rejected={report.rejected}")

    photo_image = Image.open(photo_path)
    html_path = output_dir / "evaluation.html"
    render_evaluation_html(photo_path.stem, photo_image, processed, html_path)
    json_path = output_dir / "evaluation.json"
    save_evaluation_json(photo_path.stem, processed, json_path)
    print(f"evaluation -> {html_path}")
    print(f"evaluation -> {json_path}")


def main() -> None:
    parser = argparse.ArgumentParser(description="appearance generation experiment")
    sub = parser.add_subparsers(dest="command", required=True)
    smoke_parser = sub.add_parser("smoke", help="run a smoke generation")
    smoke_parser.add_argument("--ref", type=Path, default=None, help="reference image path")
    exp_parser = sub.add_parser("experiment", help="run a full candidate experiment")
    exp_parser.add_argument("--photo", required=True, type=Path, help="reference photo path")
    exp_parser.add_argument("--count", type=int, default=4, help="number of candidates")
    exp_parser.add_argument("--subject", default="", help="subject description")
    exp_parser.add_argument(
        "--traits", type=Path, default=None, help="JSON file with locked traits"
    )
    args = parser.parse_args()
    if args.command == "smoke":
        smoke(args.ref)
    elif args.command == "experiment":
        traits = {}
        if args.traits and args.traits.exists():
            import json as _json

            traits = _json.loads(args.traits.read_text(encoding="utf-8"))
        args.traits = traits
        run_experiment(args)


if __name__ == "__main__":
    main()

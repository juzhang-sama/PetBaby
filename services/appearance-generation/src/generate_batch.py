# -*- coding: utf-8 -*-
"""Batch candidate generation with idempotent task tracking."""
import hashlib
import json
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

from PIL import Image

from identity import LockedTraits
from postprocess import remove_background
from prompt import build_prompt


@dataclass
class Candidate:
    index: int
    task_id: str
    image: Image.Image | None = None
    source_url: str | None = None
    error: str | None = None
    seed_prompt: str = ""


@dataclass
class BatchConfig:
    photo_path: Path
    traits: LockedTraits
    count: int = 4
    subject_desc: str = ""
    poll_interval: float = 5.0
    max_wait: float = 600.0


class BatchRunner:
    def __init__(self, provider, output_dir: Path):
        self.provider = provider
        self.output_dir = output_dir
        self.tasks_path = output_dir / "tasks.jsonl"
        output_dir.mkdir(parents=True, exist_ok=True)

    def _load_tasks(self) -> dict[str, dict]:
        tasks: dict[str, dict] = {}
        if self.tasks_path.exists():
            for line in self.tasks_path.read_text(encoding="utf-8").splitlines():
                if line.strip():
                    record = json.loads(line)
                    tasks[record["task_id"]] = record
        return tasks

    def _append_task(self, record: dict) -> None:
        with self.tasks_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, ensure_ascii=False) + "\n")

    @staticmethod
    def _prompt_key(prompt: str, ref_sha256: str) -> str:
        return hashlib.sha256(f"{prompt}|{ref_sha256}".encode()).hexdigest()[:16]

    def run(self, config: BatchConfig) -> list[Candidate]:
        photo = config.photo_path.read_bytes()
        ref_sha = hashlib.sha256(photo).hexdigest()
        prompt = build_prompt(
            config.subject_desc or "a pet",
            config.traits.to_prompt_block(),
        )
        existing = self._load_tasks()

        candidates: list[Candidate] = []
        for index in range(config.count):
            key = self._prompt_key(prompt, ref_sha) + f"-{index}"
            # idempotent: reuse a finished task with the same key if present
            reused = None
            for record in existing.values():
                if record.get("key") == key:
                    reused = record
                    break
            if reused and reused.get("result_url"):
                try:
                    image = Image.open(self.output_dir / "raw" / f"{reused['task_id']}.png")
                    candidates.append(
                        Candidate(
                            index=index,
                            task_id=reused["task_id"],
                            image=image,
                            source_url=reused.get("result_url"),
                            seed_prompt=prompt,
                        )
                    )
                    continue
                except OSError:
                    pass
            task_id = self.provider.submit(prompt, ref_images=[photo])
            self._append_task(
                {"task_id": task_id, "key": key, "prompt": prompt, "result_url": None}
            )
            candidates.append(
                Candidate(index=index, task_id=task_id, seed_prompt=prompt)
            )

        raw_dir = self.output_dir / "raw"
        raw_dir.mkdir(parents=True, exist_ok=True)
        finished = self._load_tasks()
        results: list[Candidate] = []
        for candidate in candidates:
            if candidate.image is not None:
                results.append(candidate)
                continue
            task_id = candidate.task_id
            try:
                deadline = time.monotonic() + config.max_wait
                while True:
                    state = self.provider.poll(task_id)
                    if state.is_final:
                        break
                    if time.monotonic() > deadline:
                        raise TimeoutError(f"task {task_id} not final after {config.max_wait}s")
                    time.sleep(config.poll_interval)
                if state.state == "success" and state.result_url:
                    image_bytes = self.provider.download(state.result_url)
                    raw_path = raw_dir / f"{task_id}.png"
                    raw_path.write_bytes(image_bytes)
                    candidate.image = Image.open(raw_path).convert("RGB")
                    candidate.source_url = state.result_url
                    record = finished.get(task_id, {})
                    record["task_id"] = task_id
                    record["result_url"] = state.result_url
                    record["state"] = "success"
                    finished[task_id] = record
                    self._rewrite_tasks(finished)
                else:
                    candidate.error = state.error or f"task ended with state={state.state}"
            except Exception as exc:  # keep going for other candidates
                candidate.error = str(exc)
            results.append(candidate)
        return results


    def _rewrite_tasks(self, records: dict[str, dict]) -> None:
        lines = "\n".join(json.dumps(r, ensure_ascii=False) for r in records.values())
        self.tasks_path.write_text(lines + "\n", encoding="utf-8")


def postprocess_candidate(image: Image.Image, method: str = "auto") -> Image.Image:
    return remove_background(image, method=method)

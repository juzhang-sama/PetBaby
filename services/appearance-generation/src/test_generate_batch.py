# -*- coding: utf-8 -*-
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import json  # noqa: E402

from generate_batch import BatchConfig, BatchRunner  # noqa: E402
from identity import LockedTraits  # noqa: E402
from provider import TaskState  # noqa: E402


class FakeProvider:
    def __init__(self):
        self.submits = 0
        self.tasks = {}
        self.png = make_png_bytes()

    def submit(self, prompt, ref_images=None, size="auto"):
        self.submits += 1
        task_id = f"t{self.submits}"
        self.tasks[task_id] = "running"
        return task_id

    def poll(self, task_id):
        if self.tasks[task_id] == "success":
            return TaskState(task_id, "success", True, result_url=f"https://x/{task_id}.png")
        self.tasks[task_id] = "success"  # finish on the next poll
        return TaskState(task_id, "running", False)

    def download(self, result_url):
        return self.png


def make_png_bytes() -> bytes:
    from PIL import Image

    buffer = __import__("io").BytesIO()
    Image.new("RGB", (32, 32), (226, 226, 226)).save(buffer, format="PNG")
    return buffer.getvalue()


def test_batch_is_idempotent_across_runs(tmp_path):
    provider = FakeProvider()
    output_dir = tmp_path / "out"
    runner = BatchRunner(provider, output_dir)
    photo = tmp_path / "photo.png"
    photo.write_bytes(make_png_bytes())
    traits = LockedTraits.from_dict(
        {"species": "cat", "fur_colors": ["grey"], "pattern": "solid"}
    )
    config = BatchConfig(
        photo_path=photo,
        traits=traits,
        count=2,
        poll_interval=0.01,
        max_wait=10,
    )
    runner.run(config)
    first_submits = provider.submits
    # second run should reuse finished raw files without re-submitting
    runner2 = BatchRunner(provider, output_dir)
    runner2.run(config)
    assert provider.submits == first_submits
    tasks = list((output_dir / "tasks.jsonl").read_text(encoding="utf-8").splitlines())
    assert len(tasks) == 2  # exactly two task records, complete with keys
    for line in tasks:
        record = json.loads(line)
        assert "task_id" in record and "key" in record

# -*- coding: utf-8 -*-
"""打包三只像素宠物为 builtin 固定帧资源（schemaVersion 7）。

源：output/中等简约像素标准验收-2026-08-21/<petId>/
目标：apps/desktop/public/builtin-pets/<petId>/
  - body.png                 （母版）
  - actions/<action>/fNN.png （逐帧）
  - manifest.json            （带真实 sha256）

帧时长来自 GIF 实测：breath=85ms、blink=150ms、tail-wag=120ms。
用法：python scripts/build-frame-sequence-manifests.py
"""
from __future__ import annotations

import hashlib
import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]  # desktop-pet
SOURCE_ROOT = ROOT / "output" / "中等简约像素标准验收-2026-08-21"
TARGET_ROOT = ROOT / "apps" / "desktop" / "public" / "builtin-pets"

PETS = [
    {
        "petId": "01-longhair-black-white",
        "displayName": "长毛黑白猫",
        "actions": [
            ("breath", 85),
            ("blink", 150),
            ("tail-wag", 120),
        ],
    },
    {
        "petId": "02-round-tabby",
        "displayName": "圆脸狸花猫",
        "actions": [
            ("breath", 85),
            ("blink", 150),
            ("tail-wag", 120),
        ],
    },
    {
        "petId": "03-sleek-black",
        "displayName": "修长黑猫",
        "actions": [
            ("breath", 85),
            ("blink", 150),
            ("tail-wag", 120),
        ],
    },
]

SEMANTICS = {
    "idle": "breath",
    "react-happy": "tail-wag",
    "react-curious": "tail-wag",
}


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def build_pet(pet: dict) -> None:
    pet_id = pet["petId"]
    source = SOURCE_ROOT / pet_id
    if not source.is_dir():
        raise SystemExit(f"missing source dir: {source}")
    target = TARGET_ROOT / pet_id
    target.mkdir(parents=True, exist_ok=True)

    # body.png = 母版
    master = source / "母版.png"
    if not master.is_file():
        raise SystemExit(f"missing master image: {master}")
    body_rel = "body.png"
    copy_file(master, target / body_rel)

    files = [{"role": "base", "relativePath": body_rel, "sha256": sha256_of(master)}]
    actions = []
    for action_id, frame_duration_ms in pet["actions"]:
        frames_dir = source / "动作" / action_id / "frames"
        if not frames_dir.is_dir():
            raise SystemExit(f"missing frames dir: {frames_dir}")
        frame_paths = sorted(frames_dir.glob("f*.png"))
        if not frame_paths:
            raise SystemExit(f"no frames in {frames_dir}")
        frame_names = []
        for index, frame_path in enumerate(frame_paths):
            rel = f"actions/{action_id}/f{index:02d}.png"
            copy_file(frame_path, target / rel)
            files.append({"role": "frame", "relativePath": rel, "sha256": sha256_of(frame_path)})
            frame_names.append(rel)
        actions.append({
            "actionId": action_id,
            "loop": True,
            "frameDurationMs": frame_duration_ms,
            "frames": frame_names,
        })

    manifest = {
        "schemaVersion": 7,
        "renderer": "frame-sequence-v1",
        "petId": pet_id,
        "variantId": "pixel-mid-simple-v1",
        "displayName": pet["displayName"],
        "species": "cat",
        "baseImage": body_rel,
        "defaultAction": "breath",
        "anchorPolicy": "fixed",
        "actions": actions,
        "semantics": SEMANTICS,
        "idleSchedule": {
            "entries": [
                {"actionId": "blink", "weight": 1, "minIntervalMs": 2500, "maxIntervalMs": 6000},
            ],
        },
        "files": files,
    }
    manifest_path = target / "manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    total_kb = sum(p.stat().st_size for p in target.rglob("*") if p.is_file()) // 1024
    frame_count = sum(len(a["frames"]) for a in actions)
    print(f"[OK] {pet_id}: {frame_count} frames, {len(files)} files, {total_kb}KB -> {target}")


def main() -> None:
    if not SOURCE_ROOT.is_dir():
        raise SystemExit(f"missing source root: {SOURCE_ROOT}")
    TARGET_ROOT.mkdir(parents=True, exist_ok=True)
    for pet in PETS:
        build_pet(pet)
    print(f"done. target root: {TARGET_ROOT}")


if __name__ == "__main__":
    sys.exit(main())

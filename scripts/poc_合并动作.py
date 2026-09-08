# -*- coding: utf-8 -*-
"""把多个动作单元合并成一个 schemaVersion-7 运行时 manifest（带真实 sha256）。

当前用途：毛砌墙 v3 的「呼吸循环 + 合成眨眼」合并为一个可被桌面端直接加载的包。

为什么必须合并而不是各放各的：
  运行时一次只加载一个 manifest，动作都必须在同一个包内，
  由 idleSchedule 决定何时播哪个（呼吸常驻循环，眨眼随机插入）。

关键参数 alignToDefaultLoop=True（硬要求）：
  合成眨眼的帧复用了呼吸 phase-0 的身体（f0000），
  如果在呼吸循环中途触发眨眼，身体会瞬间跳回 phase 0。
  运行时会把眨眼触发点对齐到呼吸循环边界（60 帧 × 42ms = 2520ms），跳变为 0。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_合并动作.py \
      --breath output/宠物动作-毛砌墙-v3-2026-08-31/04-帧序列-呼吸循环 \
      --blink  output/宠物动作-毛砌墙-v3-2026-08-31/06-眨眼/01-帧序列 \
      --out    output/宠物动作-毛砌墙-v3-2026-08-31/07-合并-呼吸眨眼 \
      --pet    "宠物动作-毛砌墙-v3-2026-08-31"
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]  # desktop-pet


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def collect_action(action_id: str, frames_dir: Path, frame_duration_ms: int, loop: bool,
                   out_dir: Path, files: list[dict], actions: list[dict],
                   base_frame: bool = False) -> None:
    """拷贝动作帧并登记到 manifest。帧名统一为 fNNNN.png，按文件名排序。

    base_frame=True 时把首帧的 role 标为 "base"（既当动作帧又当 baseImage），
    避免 files 里出现重复路径（parseFileEntries 会拒绝重复）。
    """
    if not frames_dir.is_dir():
        raise SystemExit(f"[缺输入] 动作帧目录不存在: {frames_dir}")
    frame_paths = sorted(frames_dir.glob("f*.png"))
    if not frame_paths:
        raise SystemExit(f"[缺输入] {frames_dir} 下没有 f*.png")
    rel_frames = []
    for i, src in enumerate(frame_paths):
        rel = f"frames/{action_id}/f{i:04d}.png"
        dst = out_dir / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
        role = "base" if (base_frame and i == 0) else "frame"
        files.append({"role": role, "relativePath": rel, "sha256": sha256_of(src)})
        rel_frames.append(rel)
    actions.append({
        "actionId": action_id,
        "loop": loop,
        "frameDurationMs": frame_duration_ms,
        "frames": rel_frames,
    })
    print(f"[动作] {action_id}: {len(rel_frames)} 帧 × {frame_duration_ms}ms"
          f"{' (循环)' if loop else ' (一次性)'}")


def main() -> int:
    ap = argparse.ArgumentParser(description="合并动作单元为 schemaVersion-7 manifest")
    ap.add_argument("--breath", required=True, help="呼吸循环动作目录（含 frames/）")
    ap.add_argument("--blink", required=True, help="眨眼动作目录（含 frames/）")
    ap.add_argument("--out", required=True, help="合并输出目录")
    ap.add_argument("--pet", default="宠物动作-毛砌墙-v3-2026-08-31", help="petId")
    ap.add_argument("--name", default="毛砌墙（呼吸+眨眼）", help="displayName")
    ap.add_argument("--breath-ms", type=int, default=42, help="呼吸帧时长 ms")
    ap.add_argument("--blink-ms", type=int, default=42, help="眨眼帧时长 ms")
    ap.add_argument("--blink-min", type=int, default=2500, help="眨眼最小间隔 ms")
    ap.add_argument("--blink-max", type=int, default=6000, help="眨眼最大间隔 ms")
    args = ap.parse_args()

    breath_dir = Path(args.breath)
    blink_dir = Path(args.blink)
    out_dir = Path(args.out).resolve()
    for d in (breath_dir, blink_dir):
        if not d.is_dir():
            raise SystemExit(f"[缺输入] {d}")

    if out_dir.exists():
        shutil.rmtree(out_dir)  # 重建，保证无残留
    out_dir.mkdir(parents=True, exist_ok=True)

    files: list[dict] = []
    actions: list[dict] = []

    # 呼吸循环 = defaultAction，循环；首帧同时作 baseImage（role=base）
    collect_action("breath", breath_dir / "frames", args.breath_ms, True, out_dir, files, actions,
                   base_frame=True)
    # 眨眼 = 一次性动作，由 idleSchedule 插入
    collect_action("blink", blink_dir / "frames", args.blink_ms, False, out_dir, files, actions)

    # baseImage = 呼吸锚点帧（phase-0 身体 = 眨眼帧的母体）
    base_rel = f"frames/breath/f0000.png"

    manifest = {
        "schemaVersion": 7,
        "renderer": "frame-sequence-v1",
        "petId": args.pet,
        "variantId": "breath-plus-blink",
        "displayName": args.name,
        "species": "cat",
        "baseImage": base_rel,
        "defaultAction": "breath",
        "anchorPolicy": "fixed",
        "actions": actions,
        "semantics": {"idle": "breath"},
        "idleSchedule": {
            "entries": [
                {"actionId": "blink", "weight": 1,
                 "minIntervalMs": args.blink_min, "maxIntervalMs": args.blink_max},
            ],
            # 硬要求：合成眨眼帧复用呼吸 phase-0 身体，只能对齐循环边界触发
            "alignToDefaultLoop": True,
        },
        "files": files,
        "_provenance": {
            "generator": "scripts/poc_合并动作.py",
            "breathDir": str(breath_dir),
            "blinkDir": str(blink_dir),
            "breathCycleMs": args.breath_ms * len(actions[0]["frames"]),
            "blinkDurationMs": args.blink_ms * len(actions[1]["frames"]),
            "note": "合并包。呼吸常驻循环，眨眼随机插入并对齐呼吸循环边界（跳变为 0）。",
        },
    }
    (out_dir / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")

    total_kb = sum(p.stat().st_size for p in out_dir.rglob("*") if p.is_file()) // 1024
    frame_count = sum(len(a["frames"]) for a in actions)
    print(f"\n[OK] {args.pet}: {frame_count} 帧 / {len(files)} 个文件 / {total_kb}KB")
    print(f"[输出] manifest -> {out_dir / 'manifest.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

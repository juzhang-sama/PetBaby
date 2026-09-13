# -*- coding: utf-8 -*-
"""组合循环打包：把 01-帧序列（单视频循环）打成 schemaVersion-7 运行时包。

与 poc_合并动作.py 的区别：这里只有一个 action（idle-combo，loop=true），
没有 idleSchedule —— 呼吸/眨眼/摇尾已经焊死在循环视频里，运行时纯循环播放。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_组合循环打包.py \
      --frames output/宠物动作-毛砌墙-v3-2026-08-31/09-组合循环/01-帧序列/frames \
      --out    output/宠物动作-毛砌墙-v3-2026-08-31/09-组合循环/10-运行时包 \
      --pet    "宠物动作-毛砌墙-v3-2026-08-31"
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FRAME_MS = 42

# 产品会发出的全部动作（唯一真源：apps/desktop/src/runtime/pet-presentation-controller.ts）。
# schemaVersion-7 校验器要求 semantics 显式声明每一个键：没有专属动作的就显式指向
# defaultAction。键缺失与"故意不响应"在数据上不可区分，静默回落无法被验收。
PRODUCT_MOTIONS = (
    "idle",
    "look-left",
    "look-right",
    "react-happy",
    "react-curious",
    "carried",
    "landed",
    "sleep",
    "wake",
)


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser(description="组合循环打包为 schemaVersion-7 manifest")
    ap.add_argument("--frames", required=True, help="01-帧序列/frames 目录")
    ap.add_argument("--out", required=True, help="运行时包输出目录")
    ap.add_argument("--pet", default="宠物动作-毛砌墙-v3-2026-08-31", help="petId")
    ap.add_argument("--name", default="毛砌墙（呼吸+眨眼+摇尾循环）", help="displayName")
    ap.add_argument("--action-id", default="idle-combo", help="actionId")
    ap.add_argument("--species", default="cat", choices=["cat", "dog"],
                    help="schema7 manifest 的 species 字段（上传狗必须传 dog，别让狗被记成猫）")
    args = ap.parse_args()

    frames_dir = Path(args.frames)
    out_dir = Path(args.out).resolve()
    if not frames_dir.is_dir():
        raise SystemExit(f"[缺输入] {frames_dir}")

    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    frame_paths = sorted(frames_dir.glob("f*.png"))
    if not frame_paths:
        raise SystemExit(f"[缺输入] {frames_dir} 下没有 f*.png")

    files: list[dict] = []
    rel_frames: list[str] = []
    for i, src in enumerate(frame_paths):
        rel = f"frames/{args.action_id}/f{i:04d}.png"
        dst = out_dir / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
        role = "base" if i == 0 else "frame"
        files.append({"role": role, "relativePath": rel, "sha256": sha256_of(src)})
        rel_frames.append(rel)

    manifest = {
        "schemaVersion": 7,
        "renderer": "frame-sequence-v1",
        "petId": args.pet,
        "variantId": "combo-loop-v1",
        "displayName": args.name,
        "species": "cat",
        "baseImage": f"frames/{args.action_id}/f0000.png",
        "defaultAction": args.action_id,
        "anchorPolicy": "fixed",
        "actions": [{
            "actionId": args.action_id,
            "loop": True,
            "frameDurationMs": FRAME_MS,
            "frames": rel_frames,
        }],
        "semantics": {motion: args.action_id for motion in PRODUCT_MOTIONS},
        "files": files,
        "_provenance": {
            "generator": "scripts/poc_组合循环打包.py",
            "framesDir": str(frames_dir),
            "durationMs": FRAME_MS * len(rel_frames),
            "note": "单视频循环包：呼吸+眨眼+摇尾焊死在同一循环，无 idleSchedule，运行时纯循环播放。",
        },
    }
    (out_dir / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")

    total_kb = sum(p.stat().st_size for p in out_dir.rglob("*") if p.is_file()) // 1024
    print(f"\n[OK] {args.pet}: {len(rel_frames)} 帧 / {len(files)} 文件 / {total_kb}KB")
    print(f"[输出] manifest -> {out_dir / 'manifest.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

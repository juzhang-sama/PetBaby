# -*- coding: utf-8 -*-
"""把一次性动作接入到既有 frame-sequence-v1 宠物。

适用场景：已有默认循环的宠物（idle-combo），需要追加一个 one-shot
动作（打哈欠/伸懒腰/舔毛/摇头等），自动：
    1. 复制帧到 {pet-dir}/frames/{action-name}/
    2. 计算 sha256，写入 actions 列表
    3. 写入 files 列表（base 角色 = idle-combo 首帧，保持不变）
    4. 追加/合并 idleSchedule（alignToDefaultLoop 默认 True）
    5. 写入 semantics 映射（默认 {action-name: action-name}）

不动 idle-combo 的任何文件/sha256，所以 base/已签发帧不会被改动，
下次重新打包只会改增量。

用法
----
    D:/DevTools/Python312/python.exe scripts/poc_一次性动作接入.py \
        --pet-dir apps/desktop/public/builtin-pets/04-warm-brown-tabby \
        --action-name yawn \
        --frames-dir output/.../10-打哈欠/02-帧序列/frames \
        --frame-duration-ms 42 \
        --min-interval-ms 30000 \
        --max-interval-ms 60000
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path


def sha256_of(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser(description="把一次性动作接入 frame-sequence-v1 宠物")
    ap.add_argument("--pet-dir", required=True, help="宠物目录（含 manifest.json + frames/）")
    ap.add_argument("--action-name", required=True, help="动作 id（同步作为 frames 子目录名）")
    ap.add_argument("--frames-dir", required=True, help="动作帧源目录（透明 PNG）")
    ap.add_argument("--frame-duration-ms", type=int, default=42)
    ap.add_argument("--min-interval-ms", type=int, default=30000)
    ap.add_argument("--max-interval-ms", type=int, default=60000)
    ap.add_argument("--semantic-key", default=None,
                    help="semantics 里的键名（默认与 --action-name 相同）")
    ap.add_argument("--no-align-to-default", dest="align", action="store_false",
                    help="不在默认循环边界触发")
    ap.add_argument("--replace", action="store_true",
                    help="动作已存在时替换（帧目录先清空、manifest 条目重建；调度间隔同步更新）")
    ap.add_argument("--loop", action="store_true",
                    help="循环动作（如 carried 拎起循环）；默认一次性 loop=false")
    ap.add_argument("--no-idle-schedule", action="store_true",
                    help="手动触发动作（playMotion 触发，如 carried/landed），不接入 idleSchedule")
    ap.add_argument("--existing-min-interval", type=int, default=2500,
                    help="若已有 blink 风格的 minIntervalMs，保留作为 blink 兜底（仅旧 manifest）")
    args = ap.parse_args()

    pet_dir = Path(args.pet_dir)
    manifest_path = pet_dir / "manifest.json"
    assert manifest_path.exists(), f"未找到 manifest: {manifest_path}"

    src = Path(args.frames_dir)
    src_frames = sorted(src.glob("f*.png"))
    assert src_frames, f"源目录无帧: {src}"

    # 1) 复制帧
    dst_dir = pet_dir / "frames" / args.action_name
    if dst_dir.exists():
        for p in dst_dir.glob("*.png"):
            p.unlink()
    else:
        dst_dir.mkdir(parents=True)
    for p in src_frames:
        shutil.copy2(p, dst_dir / p.name)
    print(f"[复制] {len(src_frames)} 帧 -> {dst_dir}")

    # 2) 读 manifest
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    actions = manifest.get("actions", [])
    existing = next((a for a in actions if a["actionId"] == args.action_name), None)
    if existing and not args.replace:
        print(f"[硬失败] manifest 已含 action {args.action_name!r}；"
              f"要覆盖请加 --replace")
        return 1
    replacing = existing is not None

    # 3) 构建动作
    rel_frames = [f"frames/{args.action_name}/{p.name}" for p in src_frames]
    new_action = {
        "actionId": args.action_name,
        "loop": args.loop,
        "frameDurationMs": args.frame_duration_ms,
        "frames": rel_frames,
    }
    if replacing:
        actions[actions.index(existing)] = new_action
        print(f"[替换] 覆盖已有 action {args.action_name!r} "
              f"（旧 {len(existing['frames'])} 帧 -> 新 {len(rel_frames)} 帧）")
    else:
        actions.append(new_action)
    manifest["actions"] = actions
    print(f"[动作] {args.action_name}  loop={'true' if args.loop else 'false'}  "
          f"帧数={len(rel_frames)}  frameDurationMs={args.frame_duration_ms}")

    # 4) files 列表：替换时先清掉旧条目再追加，避免残留孤儿 sha256
    files = list(manifest.get("files", []))
    if replacing:
        dropped = [f for f in files
                   if f["relativePath"].startswith(f"frames/{args.action_name}/")]
        files = [f for f in files
                 if not f["relativePath"].startswith(f"frames/{args.action_name}/")]
        print(f"[files] 清掉旧条目 {len(dropped)} 条")
    for rel in rel_frames:
        abs_p = pet_dir / rel
        files.append({"role": "frame", "relativePath": rel, "sha256": sha256_of(abs_p)})
    manifest["files"] = files
    print(f"[files] 共 {len(files)} 条")

    # 5) idleSchedule（手动触发动作如 carried/landed 跳过，不进随机调度）
    if args.no_idle_schedule:
        print("[idleSchedule] 跳过（手动触发动作，不进随机调度）")
    else:
        schedule = manifest.get("idleSchedule")
        if schedule is None:
            # 旧 manifest 可能用 blink 平铺字段（schemaVersion=5/6）。
            # 此处不强行迁移，只新增 idleSchedule。
            schedule = {"entries": [], "alignToDefaultLoop": args.align}
        if "entries" not in schedule:
            schedule["entries"] = []
        entry = next((e for e in schedule["entries"]
                      if e["actionId"] == args.action_name), None)
        if entry:
            if args.replace:
                entry.update({"weight": 1,
                              "minIntervalMs": args.min_interval_ms,
                              "maxIntervalMs": args.max_interval_ms})
                print(f"[idleSchedule] 更新 {args.action_name} 间隔 "
                      f"{args.min_interval_ms}-{args.max_interval_ms}ms")
            else:
                print(f"[WARN] idleSchedule 已含 {args.action_name}，间隔保持不变")
        else:
            schedule["entries"].append({
                "actionId": args.action_name,
                "weight": 1,
                "minIntervalMs": args.min_interval_ms,
                "maxIntervalMs": args.max_interval_ms,
            })
        schedule["alignToDefaultLoop"] = args.align
        manifest["idleSchedule"] = schedule
        print(f"[idleSchedule] alignToDefaultLoop={args.align}  "
              f"{args.action_name} 间隔 {args.min_interval_ms}-{args.max_interval_ms}ms")

    # 6) semantics
    sem_key = args.semantic_key or args.action_name
    semantics = manifest.get("semantics") or {}
    semantics[sem_key] = args.action_name
    manifest["semantics"] = semantics
    print(f"[semantics] {sem_key} -> {args.action_name}")

    # 7) 落盘
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"[输出] {manifest_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

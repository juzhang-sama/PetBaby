# -*- coding: utf-8 -*-
"""组合循环打包（薄 CLI）：把 01-帧序列 打成 schemaVersion-7 运行时包。

**逻辑已搬进服务**：`photo_avatar_backend.frames.packing.pack_frame_sequence`。
本文件只做参数解析与打印，避免脚本和服务各维护一份实现。

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
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICE_SRC = ROOT / "services" / "appearance-generation" / "src"
sys.path.insert(0, str(SERVICE_SRC))

from photo_avatar_backend.frames.packing import (  # noqa: E402
    PRODUCT_MOTIONS,
    pack_frame_sequence,
)


def main() -> int:
    ap = argparse.ArgumentParser(description="组合循环打包为 schemaVersion-7 manifest")
    ap.add_argument("--frames", required=True, help="01-帧序列/frames 目录")
    ap.add_argument("--out", required=True, help="运行时包输出目录")
    ap.add_argument("--pet", default="宠物动作-毛砌墙-v3-2026-08-31", help="petId")
    ap.add_argument("--name", default="毛砌墙（呼吸+眨眼+摇尾循环）", help="displayName")
    ap.add_argument("--action-id", default="idle-combo", help="actionId")
    ap.add_argument("--species", default="cat", choices=["cat", "dog"],
                    help="schema7 manifest 的 species 字段（上传狗必须传 dog，别让狗被记成猫）")
    ap.add_argument("--no-replace", dest="replace", action="store_false", default=True,
                    help="目标目录非空时拒绝，而不是先删掉旧帧（旧帧数百个，"
                         "原地删会被批量删除守卫拦）")
    args = ap.parse_args()

    try:
        packed = pack_frame_sequence(
            frames_dir=Path(args.frames),
            out_dir=Path(args.out),
            pet_id=args.pet,
            display_name=args.name,
            action_id=args.action_id,
            species=args.species,
            replace_existing=args.replace,
        )
    except ValueError as exc:
        raise SystemExit(f"[失败] {exc}") from exc

    total_kb = packed.total_bytes // 1024
    print(f"\n[OK] {args.pet}: {packed.frame_count} 帧 / {packed.file_count} 文件 / {total_kb}KB")
    print(f"[输出] manifest -> {packed.manifest_path}")
    print(f"[语义] semantics 覆盖 {len(PRODUCT_MOTIONS)} 键，全部指向 {args.action_id}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

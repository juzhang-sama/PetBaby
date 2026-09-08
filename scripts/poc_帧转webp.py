# -*- coding: utf-8 -*-
"""把内置宠物（04/05）的帧序列从 PNG 转 WebP，重写 manifest。

只做「转换 + 重写 manifest」，**不删除 PNG**（批量删除由调用方单独、显式执行，
避免触发批量删除守卫）。幂等：.webp 已存在则跳过。

用法：
    D:/DevTools/Python312/python.exe scripts/poc_帧转webp.py \
        --pet 04-warm-brown-tabby [--quality 90]
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
BUILTIN = ROOT / "apps/desktop/public/builtin-pets"


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def convert_one(png: Path, webp: Path, quality: int) -> int:
    img = Image.open(png).convert("RGBA")
    buf = io.BytesIO()
    img.save(buf, "WEBP", lossless=False, quality=quality, method=5)
    webp.write_bytes(buf.getvalue())
    return len(buf.getvalue())


def main() -> int:
    ap = argparse.ArgumentParser(description="内置宠物帧序列 PNG → WebP（不删除源 PNG）")
    ap.add_argument("--pet", required=True, help="petId（如 04-warm-brown-tabby）")
    ap.add_argument("--quality", type=int, default=90, help="WebP 质量 1-100（默认 90）")
    args = ap.parse_args()

    pet_dir = BUILTIN / args.pet
    manifest_path = pet_dir / "manifest.json"
    if not manifest_path.exists():
        raise SystemExit(f"[失败] 找不到 {manifest_path}")

    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    png_entries = [f for f in manifest["files"] if f["relativePath"].lower().endswith(".png")]
    if not png_entries:
        print(f"[跳过] {args.pet} 已无 .png 文件")
        return 0

    # 转换（幂等：webp 已存在则跳过）
    path_map: dict[str, str] = {}
    converted = 0
    for i, entry in enumerate(png_entries):
        rel = entry["relativePath"]
        new_rel = rel[:-4] + ".webp"
        path_map[rel] = new_rel
        png = pet_dir / rel
        webp = pet_dir / new_rel
        if not webp.exists():
            convert_one(png, webp, args.quality)
            converted += 1
        if (i + 1) % 100 == 0:
            print(f"  已处理 {i+1}/{len(png_entries)}（本次新转 {converted}）")

    # 重写 manifest（baseImage / actions[].frames[] / files[].relativePath + sha256）
    if manifest.get("baseImage") in path_map:
        manifest["baseImage"] = path_map[manifest["baseImage"]]
    for action in manifest["actions"]:
        action["frames"] = [path_map.get(f, f) for f in action["frames"]]
    for entry in manifest["files"]:
        if entry["relativePath"] in path_map:
            entry["relativePath"] = path_map[entry["relativePath"]]
            entry["sha256"] = sha256_hex((pet_dir / entry["relativePath"]).read_bytes())

    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"[完成] {args.pet}: 本次新转 {converted} 帧（共 {len(png_entries)} 帧），manifest 已重写为 .webp")
    return 0


if __name__ == "__main__":
    sys.exit(main())

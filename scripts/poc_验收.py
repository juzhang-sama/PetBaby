# -*- coding: utf-8 -*-
"""对抠像后的帧序列做四项机械验收（薄 CLI）。

**逻辑已搬进服务**：`photo_avatar_backend.frames.acceptance.accept_frames`。
本文件只做参数解析、默认路径、打印与退出码，避免脚本和服务各维护一份实现。

用法：
    D:/DevTools/Python312/python.exe scripts/poc_验收.py
    D:/DevTools/Python312/python.exe scripts/poc_验收.py --frames <目录> --out <目录>

输出：
    <out>/验收报告.json
    <out>/证据-棋盘格.png / 证据-黑底.png / 证据-白底.png
    <out>/证据-边缘放大.png / 证据-闪烁热力图.png

退出码：0 = 四项全 PASS；2 = 判定不通过（**不是崩溃**，报告已写出）。
判定不通过时按约定**先换抠像方法**，不扩展动作、不换宠物。
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICE_SRC = ROOT / "services" / "appearance-generation" / "src"
sys.path.insert(0, str(SERVICE_SRC))

from photo_avatar_backend.frames.acceptance import (  # noqa: E402
    AcceptanceError,
    accept_frames,
)

POC_DIR = ROOT / "output" / "POC-绿幕视频-2026-08-29"
DEFAULT_FRAMES = POC_DIR / "04-帧序列" / "frames"
OUT_DIR = POC_DIR / "05-验收"


def main() -> int:
    parser = argparse.ArgumentParser(description="四项机械验收")
    parser.add_argument("--frames", type=str, default=None, help="帧序列目录")
    parser.add_argument("--out", type=str, default=None, help="验收输出目录")
    args = parser.parse_args()

    frames_dir = Path(args.frames) if args.frames else DEFAULT_FRAMES
    out_dir = Path(args.out) if args.out else OUT_DIR

    try:
        result = accept_frames(frames_dir, out_dir, path_base=ROOT)
    except AcceptanceError as exc:
        raise SystemExit(f"{exc}\n         先运行 scripts/poc_抠像.py") from exc

    print()
    for name, item in result.criteria.items():
        print(f"  {name}:")
        for key, value in item.items():
            if key in ("passed", "note", "thresholds"):
                continue
            print(f"    {key}: {value}")
    print()
    print(f"总体判定: {'通过' if result.overall_passed else '不通过'}")
    print(f"报告: {result.report_path}")
    if not result.overall_passed:
        print(f"\n[FAIL] {', '.join(result.failed_criteria)} —— 先看证据图，再按约定换抠像模型：")
        print("    D:/DevTools/Python312/python.exe scripts/poc_抠像.py --method sam2")
        print("    不要扩展动作，不要换宠物。")
    return 0 if result.overall_passed else 2


if __name__ == "__main__":
    sys.exit(main())

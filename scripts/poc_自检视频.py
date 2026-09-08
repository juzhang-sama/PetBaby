# -*- coding: utf-8 -*-
"""POC 工具：合成一支"假的"绿幕测试视频，用于自检抠像工具链。

用途：在真实 Seedance 视频到位之前，先验证
    抽帧 -> 色键 -> 去绿 -> 平滑 -> 帧序列 -> manifest
整条链路能跑通。它不是验收样本，产物只用于自检。

刻意使用 libx264 + yuv420p 编码，制造与真实平台一致的色度二次采样损伤，
这样自检结果才有参考价值。

用法：
    D:/DevTools/Python312/python.exe scripts/poc_自检视频.py
输出：
    output/POC-绿幕视频-2026-08-29/00-自检/自检视频.mp4
"""
from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
POC_DIR = ROOT / "output" / "POC-绿幕视频-2026-08-29"
OUT_DIR = POC_DIR / "00-自检"

MASTER = POC_DIR / "01-首帧" / "母版-1024.png"
GREEN = (0, 255, 0)
FPS = 24
SECONDS = 5
BREATH_CYCLES = 2.0          # 5 秒内 2 次呼吸
BREATH_X = 0.014             # 横向起伏幅度（相对比例）
BREATH_Y = 0.005

FFMPEG = shutil.which("ffmpeg") or "ffmpeg"


def build_frames() -> list[Image.Image]:
    master = Image.open(MASTER).convert("RGBA")
    alpha = np.array(master)[:, :, 3]
    ys, xs = np.nonzero(alpha >= 128)
    # 锚点固定在底部中心：模拟"脚掌不动、胸腔起伏"
    ax = float((xs.min() + xs.max()) / 2.0)
    ay = float(ys.max())

    total = FPS * SECONDS
    frames = []
    for i in range(total):
        phase = 2.0 * np.pi * BREATH_CYCLES * (i / total)
        sx = 1.0 + BREATH_X * np.sin(phase)
        sy = 1.0 + BREATH_Y * np.sin(phase)
        # PIL AFFINE: x' = a*x + b*y + c ; y' = d*x + e*y + f
        warped = master.transform(
            master.size, Image.AFFINE,
            (sx, 0.0, ax * (1.0 - sx), 0.0, sy, ay * (1.0 - sy)),
            resample=Image.BICUBIC,
        )
        bg = Image.new("RGBA", master.size, GREEN + (255,))
        frames.append(Image.alpha_composite(bg, warped).convert("RGB"))
    return frames


def main() -> int:
    if not MASTER.is_file():
        raise SystemExit(f"[缺输入] 先运行 scripts/poc_绿幕首帧.py 生成 {MASTER}")
    if OUT_DIR.exists():
        shutil.rmtree(OUT_DIR)
    raw_dir = OUT_DIR / "raw"
    raw_dir.mkdir(parents=True, exist_ok=True)

    frames = build_frames()
    for i, frame in enumerate(frames):
        frame.save(raw_dir / f"s{0 + i:04d}.png")
    print(f"[生成] {len(frames)} 帧 -> {raw_dir}")

    out = OUT_DIR / "自检视频.mp4"
    cmd = [FFMPEG, "-y", "-framerate", str(FPS), "-i", str(raw_dir / "s%04d.png"),
           "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18",
           "-preset", "medium", str(out)]
    result = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace")
    if result.returncode != 0:
        print(result.stderr[-2000:])
        return 1
    print(f"[编码] {out}  ({out.stat().st_size // 1024} KB)")
    print("[说明] 这是合成的假视频，仅用于自检工具链，不作为验收样本")
    print("\n[下一步] 用它跑通链路：")
    print(f"    D:/DevTools/Python312/python.exe scripts/poc_抠像.py --video \"{out.as_posix()}\"")
    return 0


if __name__ == "__main__":
    sys.exit(main())

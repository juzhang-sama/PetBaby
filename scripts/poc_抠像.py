# -*- coding: utf-8 -*-
"""绿幕视频自动抠像（薄 CLI）。

**逻辑已搬进服务**：`photo_avatar_backend.frames.matting.matte_video`。
本文件只做参数解析、默认路径与打印，避免脚本和服务各维护一份实现。

用法：
    D:/DevTools/Python312/python.exe scripts/poc_抠像.py
    D:/DevTools/Python312/python.exe scripts/poc_抠像.py --method chroma --fps 24
    D:/DevTools/Python312/python.exe scripts/poc_抠像.py --video <路径> --temporal 5

输出：
    <out>/frames/f000.png ...
    <out>/body.png            （首帧静态底图）
    <out>/抠像参数.json        （被 对齐 / 体检 / 预览 三个脚本消费，别改字段名）

方法说明：
    chroma  色键 + 反算去绿 + 形态学清理 + 时间平滑（默认，无需下载模型）
    rvm     预留接口：Robust Video Matting（人像模型，宠物需实测）
    sam2    预留接口：视频分割（需下载权重）

第一轮若「不通过」，按约定先换 --method，不扩展动作和宠物类型。
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICE_SRC = ROOT / "services" / "appearance-generation" / "src"
sys.path.insert(0, str(SERVICE_SRC))

from photo_avatar_backend.frames.matting import WARMUP_TOL, matte_video  # noqa: E402

POC_DIR = ROOT / "output" / "POC-绿幕视频-2026-08-29"
VIDEO_DIR = POC_DIR / "03-视频"
OUT_DIR = POC_DIR / "04-帧序列"


def main() -> int:
    parser = argparse.ArgumentParser(description="绿幕视频自动抠像 -> 透明 PNG 帧序列")
    parser.add_argument("--video", type=str, default=None, help="输入视频路径")
    parser.add_argument("--method", choices=["chroma", "rvm", "sam2"], default="chroma")
    parser.add_argument("--fps", type=float, default=0, help="抽帧帧率，0 表示跟随源视频")
    parser.add_argument("--size", type=int, default=0, help="输出边长，0 表示保持源尺寸")
    parser.add_argument("--temporal", type=int, default=0,
                        help="时间平滑半径（帧），默认 0 关闭；绿幕脏、色键发抖时再开（用中位数）")
    parser.add_argument("--spatial", type=float, default=0.6, help="alpha 空间平滑 sigma")
    parser.add_argument("--frame-duration-ms", type=int, default=0, help="帧时长，0=按 fps 推算")
    parser.add_argument("--out", type=str, default=None,
                        help="输出目录，默认 04-帧序列；自检时请指向 00-自检/帧序列")
    parser.add_argument("--autocrop", dest="autocrop", action="store_true", default=True,
                        help="按全部帧前景并集自动裁正方形（默认开）")
    parser.add_argument("--no-autocrop", dest="autocrop", action="store_false",
                        help="关闭自动取景，保留原始画幅")
    parser.add_argument("--auto-warmup", dest="auto_warmup", action="store_true", default=True,
                        help="自动检测并跳过开头背景未稳定的过渡帧（默认开）")
    parser.add_argument("--no-warmup", dest="auto_warmup", action="store_false",
                        help="关闭过渡帧检测")
    parser.add_argument("--warmup-tol", type=float, default=WARMUP_TOL,
                        help="背景稳定判定容差（绿幕 G 值）")
    parser.add_argument("--color-match", type=str, default=None,
                        help="母版 PNG 路径；以它的主体颜色为目标校正成品（修 Seedance 改色）")
    args = parser.parse_args()

    out_dir = Path(args.out) if args.out else OUT_DIR

    video = Path(args.video) if args.video else None
    if video is None:
        candidates = (sorted(VIDEO_DIR.glob("*.mp4")) + sorted(VIDEO_DIR.glob("*.mov"))
                      + sorted(VIDEO_DIR.glob("*.webm")))
        if not candidates:
            raise SystemExit(
                f"[缺输入] 请把 Seedance 生成的视频放到：{VIDEO_DIR}\n"
                f"        或用 --video 指定路径"
            )
        video = candidates[0]

    try:
        result = matte_video(
            video,
            out_dir,
            method=args.method,
            fps=args.fps,
            size=args.size,
            temporal=args.temporal,
            spatial=args.spatial,
            frame_duration_ms=args.frame_duration_ms,
            autocrop=args.autocrop,
            auto_warmup=args.auto_warmup,
            warmup_tol=args.warmup_tol,
            color_match=Path(args.color_match) if args.color_match else None,
            path_base=ROOT,
        )
    except ValueError as exc:
        raise SystemExit(f"[失败] {exc}") from exc

    print(f"[输出] manifest（抠像参数）: {result.params_path}")
    print("\n[下一步] 运行验收：")
    print("    D:/DevTools/Python312/python.exe scripts/poc_验收.py "
          f'--frames "{result.frames_dir}"')
    return 0


if __name__ == "__main__":
    sys.exit(main())

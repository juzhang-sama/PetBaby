# -*- coding: utf-8 -*-
"""组合预览：呼吸循环 + 偶发动作（眨眼）穿插播放。

为什么要单独做这个预览：
  眨眼是"插在呼吸里的偶发动作"，单看 4 帧眨眼序列没有意义 ——
  真实观感取决于两件事：①眨眼发生时身体是否跳动；②眨完回到呼吸是否衔接。
  这两个都只有在"呼吸+眨眼"连续播放时才能看出来。

产物是自包含 HTML + 两张精灵图，双击即可打开。

用法（单次）：
  D:/DevTools/Python312/python.exe scripts/poc_预览组合.py \
      --base   output/.../04-帧序列-呼吸循环/frames \
      --insert output/.../06-眨眼/01-帧序列/frames \
      --label  眨眼 \
      --out    output/.../06-眨眼

用法（多版本对比，共享一张呼吸精灵图，每版出一张 HTML）：
  D:/DevTools/Python312/python.exe scripts/poc_预览组合.py \
      --base   output/.../04-帧序列-呼吸循环/frames \
      --insert output/.../06-眨眼/01-帧序列/frames \
               output/.../06-眨眼/02-帧序列-8帧-336ms/frames \
               output/.../06-眨眼/03-帧序列-12帧-504ms/frames \
      --label  眨眼 \
      --out    output/.../06-眨眼/_预览对比

  多版本时放大窗口取各版变化区域的并集，保证三页看的是同一块裁剪。
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
MAX_SHEET = 4096
TEMPLATE = ROOT / "scripts" / "_预览模板.html"


def load_template(name: str = "_预览模板.html") -> str:
    """HTML 模板外置在 scripts/ 下，用 __XXX__ 显式占位符。

    外置原因：模板含大量 JS 花括号，内联进 str.format() 需要全部转义，极易漏改。
    """
    p = ROOT / "scripts" / name
    if not p.is_file():
        raise SystemExit(f"[缺模板] {p}")
    return p.read_text(encoding="utf-8")


def render_html(tpl: str, mapping: dict) -> str:
    """用显式占位符替换（__XXX__），避免 str.format 吃掉 JS 的花括号。"""
    out = tpl
    for k, v in mapping.items():
        out = out.replace(f"__{k}__", str(v))
    left = re.findall(r"__[A-Z]+__", out)
    if left:
        raise SystemExit(f"[模板未替换] 残留占位符：{sorted(set(left))}")
    return out


def diff_bbox(anchor: Path, action_frames: list[Path], thresh: int = 8,
              pad: int = 6) -> tuple[dict, float]:
    """自动反推「动作实际改变了哪块区域」（归一化 bbox），供预览页做放大窗口。

    只用 RGB 通道比较：动作帧是「锚点帧 + 局部 patch」，alpha 应当完全一致，
    拿 alpha 比较会得到空 bbox。
    透明区域的 RGB 是无意义垃圾值，必须先按 alpha 掩掉，否则 bbox 会铺满画布。
    """
    a = np.asarray(Image.open(anchor).convert("RGBA")).astype(np.int16)
    h, w = a.shape[:2]
    acc = np.zeros((h, w), dtype=bool)
    for p in action_frames:
        b = Image.open(p).convert("RGBA")
        if b.size != (w, h):
            b = b.resize((w, h), Image.NEAREST)
        b = np.asarray(b).astype(np.int16)
        d = np.abs(b[:, :, :3] - a[:, :, :3]).max(axis=2)
        vis = np.maximum(a[:, :, 3], b[:, :, 3]) > 200
        acc |= (d > thresh) & vis
    if not acc.any():
        # 没检测到变化：退回画布中部窗口，页面至少不崩，并在标题里能看出来
        return {"x": 0.30, "y": 0.20, "w": 0.40, "h": 0.30}, 0.0
    ys, xs = np.nonzero(acc)
    x0 = max(0, int(xs.min()) - pad)
    x1 = min(w, int(xs.max()) + 1 + pad)
    y0 = max(0, int(ys.min()) - pad)
    y1 = min(h, int(ys.max()) + 1 + pad)
    pct = 100.0 * acc.sum() / (h * w)
    return {"x": round(x0 / w, 4), "y": round(y0 / h, 4),
            "w": round((x1 - x0) / w, 4), "h": round((y1 - y0) / h, 4)}, pct


def union_bbox(boxes: list[dict]) -> dict:
    """取多个归一化 bbox 的并集 —— 多版本对比时保证放大窗口一致。"""
    x0 = min(b["x"] for b in boxes)
    y0 = min(b["y"] for b in boxes)
    x1 = max(b["x"] + b["w"] for b in boxes)
    y1 = max(b["y"] + b["h"] for b in boxes)
    return {"x": round(x0, 4), "y": round(y0, 4),
            "w": round(x1 - x0, 4), "h": round(y1 - y0, 4)}


def build_sheet(frames: list[Path], cell: int, out_path: Path) -> tuple[int, int]:
    n = len(frames)
    cols = max(1, min(n, MAX_SHEET // cell))
    rows = int(np.ceil(n / cols))
    sheet = Image.new("RGBA", (cols * cell, rows * cell), (0, 0, 0, 0))
    for i, p in enumerate(frames):
        im = Image.open(p).convert("RGBA").resize((cell, cell), Image.LANCZOS)
        sheet.paste(im, ((i % cols) * cell, (i // cols) * cell))
    sheet.save(out_path)
    return cols, rows


def main() -> int:
    ap = argparse.ArgumentParser(description="组合预览：呼吸 + 偶发动作")
    ap.add_argument("--base", required=True, help="基础循环帧目录（呼吸）")
    ap.add_argument("--insert", nargs="+", required=True,
                    help="偶发动作帧目录（眨眼），可传多个做多版本对比")
    ap.add_argument("--label", default="眨眼")
    ap.add_argument("--out", required=True, help="输出目录")
    ap.add_argument("--cell", type=int, default=256)
    ap.add_argument("--disp", type=int, default=512)
    ap.add_argument("--frame-ms", type=int, default=42)
    # v1 的默认值是 3（=3 个呼吸周期 ≈7.56 秒），动作本身只有 168ms，
    # 占空比 2.8% —— 打开页面基本看不到动作。默认改 1 个周期（≈2.5 秒）。
    ap.add_argument("--interval", type=int, default=1, help="默认间隔（呼吸周期数）")
    ap.add_argument("--zoom", type=int, default=4, help="眼部放大倍数")
    args = ap.parse_args()

    base_dir = Path(args.base)
    ins_dirs = [Path(p) for p in args.insert]
    out_dir = Path(args.out).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    for d in [base_dir, *ins_dirs]:
        if not d.is_dir():
            raise SystemExit(f"[缺输入] {d}")

    base_frames = sorted(base_dir.glob("f*.png"))
    if not base_frames:
        raise SystemExit("[缺输入] 呼吸帧目录为空")
    versions: list[tuple[str, list[Path]]] = []
    for d in ins_dirs:
        fr = sorted(d.glob("f*.png"))
        if not fr:
            raise SystemExit(f"[缺输入] 动作帧目录为空：{d}")
        versions.append((d.parent.name, fr))

    base_sheet = out_dir / "预览-呼吸精灵图.png"
    bcols, _ = build_sheet(base_frames, args.cell, base_sheet)
    print(f"[精灵图] 呼吸 {len(base_frames)} 帧 {bcols}列 → {base_sheet.name}")

    sample = Image.open(base_frames[0])
    cycle_sec = len(base_frames) * args.frame_ms / 1000.0

    # 放大窗口：先各版自算 bbox，再取并集 —— 多版本对比必须看同一块裁剪，
    # 否则窗口一大一小时，快慢差异会和裁剪差异混在一起，没法判断。
    boxes, pcts = [], []
    for tag, fr in versions:
        box, pct = diff_bbox(base_frames[0], fr)
        boxes.append(box)
        pcts.append(pct)
        print(f"[变化区] {tag}: bbox={box} 占画布 {pct:.2f}%")
    eye = union_bbox(boxes)
    print(f"[放大窗] 并集 bbox={eye}")

    eye_w = max(64, int(args.disp * eye["w"] * args.zoom))
    eye_h = max(64, int(args.disp * eye["h"] * args.zoom))

    tpl = load_template()
    built: list[dict] = []
    for (tag, fr), pct in zip(versions, pcts):
        ins_sheet = f"预览-{args.label}精灵图-{tag}.png"
        icols, _ = build_sheet(fr, args.cell, out_dir / ins_sheet)
        print(f"[精灵图] {tag} {len(fr)} 帧 {icols}列 → {ins_sheet}")
        built.append({"tag": tag, "sheet": ins_sheet, "cols": icols, "n": len(fr)})

        cfg = {
            "cell": args.cell,
            "baseCols": bcols,
            "insCols": icols,
            "baseSheet": base_sheet.name,
            "insSheet": ins_sheet,
            "baseN": len(base_frames),
            "insN": len(fr),
            "frameMs": args.frame_ms,
            "cycleSec": cycle_sec,
        }
        html = render_html(tpl, {
            "CONFIG": json.dumps(cfg, ensure_ascii=False),
            "LABEL": args.label,
            "EYE": json.dumps(eye, ensure_ascii=False),
            "ZOOM": args.zoom,
            "DISP": args.disp,
            "EYEW": eye_w,
            "EYEH": eye_h,
            "DEFIV": args.interval,
            "DEFIVSEC": f"{args.interval * cycle_sec:.1f}",
            "FRAMEMS": args.frame_ms,
            "BN": len(base_frames),
            "CYC": cycle_sec,
            "INN": len(fr),
            "INDUR": len(fr) * args.frame_ms,
            "W": sample.size[0],
            "H": sample.size[1],
            "EYEPCT": f"{pct:.2f}",
        })
        html_path = out_dir / f"预览-{args.label}-{tag}.html"
        html_path.write_text(html, encoding="utf-8")
        print(f"[输出] {html_path}  （{len(fr)} 帧 / {len(fr) * args.frame_ms}ms）")

    # 多版本时额外出一张并排同步页：三版同时触发、同时推进帧号，
    # 帧数少的先播完先回呼吸 —— 快慢差异一眼可见，比开三个标签页来回切强得多。
    if len(built) > 1:
        cfg_cmp = {
            "cell": args.cell,
            "baseCols": bcols,
            "baseSheet": base_sheet.name,
            "baseN": len(base_frames),
            "frameMs": args.frame_ms,
            "cycleSec": cycle_sec,
            "versions": built,
        }
        html = render_html(load_template("_预览模板-并排.html"), {
            "CONFIG": json.dumps(cfg_cmp, ensure_ascii=False),
            "LABEL": args.label,
            "EYE": json.dumps(eye, ensure_ascii=False),
            "ZOOM": args.zoom,
            "DISP": args.disp,
            "EYEW": eye_w,
            "EYEH": eye_h,
            "FRAMEMS": args.frame_ms,
            "BN": len(base_frames),
            "CYC": cycle_sec,
            "W": sample.size[0],
            "H": sample.size[1],
            "EYEPCT": f"{max(pcts):.2f}",
            "NV": len(built),
            "DEFIV": args.interval,
        })
        cmp_path = out_dir / f"预览-{args.label}-{len(built)}版并排.html"
        cmp_path.write_text(html, encoding="utf-8")
        print(f"[输出] {cmp_path}  ★ 并排同步对比，推荐先看这个")

    print(f"[输出] {base_sheet}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

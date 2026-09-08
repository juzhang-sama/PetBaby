# -*- coding: utf-8 -*-
"""96 网格动作验证：把母版降到 96 网格，跑呼吸/眨眼/摇尾巴，看动作是否还正常。

不改正式模块，用 monkey-patch 临时把 LOGICAL_GRID_SIZE 从 160 降到 96。
呼吸测两个峰值（1px / 2px），看档位够不够、顺滑度还在不在。
"""
from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
from PIL import Image

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

import 中等简约动作 as MA  # noqa: E402
import 中等简约尾巴 as MT  # noqa: E402
from 中等简约动作 import MotionAnnotation, audit_action, ActionAuditSpec  # noqa: E402
from 中等简约产物 import load_logical_rgba  # noqa: E402

BASE = SCRIPTS.parent / "output" / "中等简约像素标准验收-2026-08-21"
OUT = SCRIPTS.parent / "output" / "96网格动作验证-2026-08-24"
OUT.mkdir(parents=True, exist_ok=True)

GRID_OLD = 160
GRID_NEW = 96
SCALE = GRID_NEW / GRID_OLD  # 0.6


def scale_rect(r):
    return tuple(int(round(v * SCALE)) for v in r)


def scale_point(p):
    return tuple(int(round(v * SCALE)) for v in p)


def patch_grid():
    MA.LOGICAL_GRID_SIZE = GRID_NEW
    MA._tail_mask.__defaults__ = (GRID_NEW,)
    MT.LOGICAL_GRID_SIZE = GRID_NEW


def load_annotation_96(pid: str) -> MotionAnnotation:
    raw = (BASE / "annotations" / f"{pid}.json").read_text(encoding="utf-8")
    import json
    d = json.loads(raw)
    d["logicalGridSize"] = 160  # 字段 Literal[160]，坐标已是 96 网格，字段值保留
    d["eyes"] = {k: list(scale_rect(v)) for k, v in d["eyes"].items()}
    d["breathZone"] = list(scale_rect(d["breathZone"]))
    d["groundAnchors"] = [list(scale_rect(r)) for r in d["groundAnchors"]]
    d["tail"]["root"] = list(scale_point(d["tail"]["root"]))
    d["tail"]["mask"] = [list(scale_point(p)) for p in d["tail"]["mask"]]
    return MotionAnnotation.model_validate(d)


def load_source_96(pid: str) -> np.ndarray:
    a = load_logical_rgba(BASE / pid / "母版.png")  # 160 网格
    img = Image.fromarray(a, "RGBA").resize((GRID_NEW, GRID_NEW), Image.Resampling.NEAREST)
    return np.asarray(img, dtype=np.uint8)


def upscale(frames, factor=8):
    out = []
    for f in frames:
        out.append(Image.fromarray(f, "RGBA").resize(
            (GRID_NEW * factor, GRID_NEW * factor), Image.Resampling.NEAREST))
    return out


def save_gif(frames, path, duration):
    bg = Image.new("RGBA", (GRID_NEW, GRID_NEW), (236, 234, 229, 255))
    review = [Image.alpha_composite(bg, Image.fromarray(f, "RGBA"))
              .convert("P", palette=Image.Palette.ADAPTIVE) for f in frames]
    review[0].save(path, save_all=True, append_images=review[1:],
                   duration=duration, loop=0, disposal=2)


def main():
    patch_grid()
    for pid in ["01-longhair-black-white", "02-round-tabby", "03-sleek-black"]:
        src = load_source_96(pid)
        ann = load_annotation_96(pid)

        # 呼吸：两个峰值
        for peak in (1, 2):
            MA.BREATH_PEAK_PIXELS = peak
            frames = MA.make_breath(src, ann)
            # 档位统计（每侧 half 值）
            halves = set()
            for f in frames:
                diff = np.any(f != src, axis=2)
                halves.add(int(diff.sum()))
            audit = audit_action(ActionAuditSpec("breath", ann, ann.breath_zone), src, frames)
            save_gif(frames, OUT / f"{pid}-呼吸-峰值{peak}px.gif", 85)
            print(f"{pid} 呼吸峰值{peak}px: 帧={len(frames)} 闭合={audit.loop_closed} "
                  f"锚点={audit.ground_anchor_changed_pixels} 峰值变化={audit.peak_changed_pixels}px")

        # 眨眼
        frames = MA.make_blink(src, ann)
        audit = audit_action(ActionAuditSpec("blink", ann, ann.eye_bounds), src, frames)
        save_gif(frames, OUT / f"{pid}-眨眼.gif", 150)
        print(f"{pid} 眨眼: 帧={len(frames)} 闭合={audit.loop_closed} 峰值变化={audit.peak_changed_pixels}px")

        # 摇尾巴
        try:
            frames = MA.make_tail_wag(src, ann)
            audit = audit_action(ActionAuditSpec("tail-wag", ann, ann.tail.bounds), src, frames)
            save_gif(frames, OUT / f"{pid}-摇尾巴.gif", 120)
            print(f"{pid} 摇尾巴: 帧={len(frames)} 闭合={audit.loop_closed} 峰值变化={audit.peak_changed_pixels}px")
        except Exception as e:
            print(f"{pid} 摇尾巴失败: {e}")

    print(f"\n输出目录: {OUT}")


if __name__ == "__main__":
    main()

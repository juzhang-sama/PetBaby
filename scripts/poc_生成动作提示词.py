# -*- coding: utf-8 -*-
"""生成偶发动作的 Seedance 提示词：骨架 + 动作配置 + 宠物档案 = 完整提示词。

为什么需要这个脚本
------------------
早期做法是"写好一份提示词，复制一份改猫的品种描述"。问题：
两份文件 90% 相同却各自维护，改一处必漏另一处。实测已经出现苗头——
yawn 提示词里一段过时规则只存在于毛砌墙那份，改的时候才发现建国那份根本没有。

提示词里真正因猫而异的只有三块：
    1. 身份描述（虎斑 / 银渐层的品种特征）
    2. 毛色描述 + 该毛色的漂移方向（暖棕怕变灰变巧克力、银渐层怕变蓝灰发黄）
    3. 取景框余量数字（每只猫首帧 scale 不同，crop box 和余量都不同）

其余（绿幕、镜头、首尾一致、动作时间轴、负向词骨架）完全通用。
所以拆成三层，各自独立演进：

    scripts/_提示词模板/骨架.txt     公共骨架（含占位符）
    scripts/_提示词模板/<action>.json  动作配置（时间轴/细节/严格/负向/包络风险）
    output/宠物档案/<petId>.json     宠物变量（身份/毛色/crop/锚点帧）

加新宠物 = 加一个宠物档案 JSON；加新动作 = 加一个动作 JSON。都不用碰骨架。

用法
----
    # 单只宠物
    D:/DevTools/Python312/python.exe scripts/poc_生成动作提示词.py \
        --pet 04-warm-brown-tabby --action lick --serial 07 \
        --out output/宠物动作-毛砌墙-v3-2026-08-31/02-提示词/Seedance提示词-07-舔毛.txt

    # 所有宠物一次生成（推荐：改一处模板 → N 份同步，避免手敲漏改）
    D:/DevTools/Python312/python.exe scripts/poc_生成动作提示词.py \
        --all-pets --action grab-release --serial 12 --subdir 12-拎起

    # 列出可用的宠物档案与动作配置
    D:/DevTools/Python312/python.exe scripts/poc_生成动作提示词.py --list

为什么物理上是 N 个文件而不是 1 个
--------------------------------
维护单元只有 1 份（scripts/_提示词模板/<action>.json + 骨架.txt），
但 Seedance 必须拿到填好具体变量的文本——身份描述、毛色、crop 余量、
文件路径这些因猫而异，没法写成占位符丢给平台。
所以 N 个文件 = 同一份模板的 N 次渲染，不是 N 份各自维护的提示词。
用 --all-pets 可以保证「改一处 → 一条命令 → N 份同步」。
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEMPLATE_DIR = ROOT / "scripts" / "_提示词模板"
SKELETON = TEMPLATE_DIR / "骨架.txt"
PROFILE_DIR = ROOT / "output" / "宠物档案"

# 骨架里允许出现的占位符（其余占位符一律报错，防止拼写错误静默通过）
ALLOWED = {
    "__SERIAL__", "__ACTION_NAME__", "__ACTION_ID__", "__ACTION_SECTION__",
    "__ACTION_DETAILS__", "__ACTION_STRICT__", "__ACTION_NEGATIVE__",
    "__ACTION_ENVELOPE_RISK__", "__ACTION_REVIEW_HEADLINE__", "__DURATION__",
    "__PET_EXTRA__",
    "__DISPLAY_NAME__", "__MASTER_TASK_ID__", "__FIRST_FRAME__",
    "__IDENTITY__", "__COAT__", "__COAT_GUARD__", "__COAT_NEGATIVE__",
    "__REVIEW_COAT_HINT__", "__CROP_DESC__", "__SOURCE_SIZE__", "__OUTPUT_SIZE__",
    "__UNION_X0__", "__UNION_Y0__", "__UNION_X1__", "__UNION_Y1__",
    "__MARGIN_TOP__", "__MARGIN_BOTTOM__", "__MARGIN_LEFT__", "__MARGIN_RIGHT__",
    "__ANCHOR_FRAME__", "__REF_PARAMS__", "__PET_DIR__",
    "__ARCH_NOTE__", "__TAIL_HEADER__", "__TAIL_REQ__", "__GEN_MODE__",
    "__LAST_FRAME_LINE__", "__PIPELINE_TAIL__", "__RUNTIME_WIRING__",
    "__ACTION_TYPE__", "__REVIEW_TONGUE_LINE__", "__REVIEW_EYE_LINE__",
}


def load_json(p: Path) -> dict:
    return json.loads(p.read_text(encoding="utf-8"))


def margins(crop: dict) -> dict:
    """按 crop box 与内容并集算出四边余量（源画幅空间）。"""
    x0, y0, x1, y1 = crop["contentUnion"]
    cx, cy, size = crop["x"], crop["y"], crop["size"]
    return {
        "top": y0 - cy,
        "bottom": (cy + size - 1) - y1,
        "left": x0 - cx,
        "right": (cx + size - 1) - x1,
    }


def build(pet: dict, action: dict, serial: str) -> str:
    pet_extra = action.get("petExtra", {}).get(pet["petId"], "")
    crop = pet["crop"]
    m = margins(crop)
    if min(m.values()) < 0:
        raise SystemExit(f"[硬失败] 内容并集已超出 crop box，余量={m}；"
                         f"先更新 {pet['petId']} 档案里的 crop，别硬生成")
    size = crop["size"]
    out_size = crop["outputSize"]
    crop_desc = (f"crop({crop['x']},{crop['y']},{size})→{out_size}"
                 + ("" if size == out_size else f"（再缩放 {out_size/size:.4f}）"))

    values = {
        "__SERIAL__": serial,
        "__ACTION_NAME__": action["actionName"],
        "__ACTION_ID__": action["actionId"],
        "__ACTION_SECTION__": action["section"],
        "__ACTION_DETAILS__": action["details"],
        "__ACTION_STRICT__": action["strict"],
        "__ACTION_NEGATIVE__": action["negative"],
        "__ACTION_ENVELOPE_RISK__": action["envelopeRisk"],
        "__ACTION_REVIEW_HEADLINE__": action["reviewHeadline"],
        "__DURATION__": str(action["duration"]),
        # 宠物专属追加约束（例如毛砌墙首帧垂直余量只有建国一半，必须额外约束下沉幅度）。
        # 没有配置就是空串，空占位符所在行会被下面的空行压缩掉。
        "__PET_EXTRA__": (pet_extra + "\n") if pet_extra else "",
        "__DISPLAY_NAME__": pet["displayName"],
        "__MASTER_TASK_ID__": str(pet["masterTaskId"]),
        "__FIRST_FRAME__": pet["firstFrame"],
        "__IDENTITY__": pet["identity"],
        "__COAT__": pet["coat"],
        "__COAT_GUARD__": pet["coatGuard"],
        "__COAT_NEGATIVE__": pet["coatNegative"],
        "__REVIEW_COAT_HINT__": pet["reviewCoatHint"],
        "__CROP_DESC__": crop_desc,
        "__SOURCE_SIZE__": str(crop["sourceSize"]),
        "__OUTPUT_SIZE__": str(out_size),
        "__UNION_X0__": str(crop["contentUnion"][0]),
        "__UNION_Y0__": str(crop["contentUnion"][1]),
        "__UNION_X1__": str(crop["contentUnion"][2]),
        "__UNION_Y1__": str(crop["contentUnion"][3]),
        "__MARGIN_TOP__": str(m["top"]),
        "__MARGIN_BOTTOM__": str(m["bottom"]),
        "__MARGIN_LEFT__": str(m["left"]),
        "__MARGIN_RIGHT__": str(m["right"]),
        "__ANCHOR_FRAME__": pet["runtime"]["anchorFrame"],
        "__REF_PARAMS__": pet["runtime"]["refParams"],
        "__PET_DIR__": pet["runtime"]["petDir"],
        # 以下默认值 = 偶发动作行为；交互动作（grab-release 等）在 action json 里覆盖
        "__ARCH_NOTE__": action.get("archNote",
            "偶发动作，独立视频 + idleSchedule 随机插入，不做循环常驻成员。"),
        "__TAIL_HEADER__": action.get("tailHeader", "首尾一致性"),
        "__TAIL_REQ__": action.get("tailReq",
            "第 0 秒和第 %s 秒，猫的端坐姿态、头部角度、耳朵角度、眼睛睁开程度必须高度一致\n"
            "（不强求像素级一致，因为是偶发动作而非循环视频）。\n"
            "下一段 idle-combo 循环视频的衔接由运行时 idleSchedule 处理。"
            % str(action["duration"])),
        "__GEN_MODE__": action.get("genMode", "图生视频（首帧即可，不需要尾帧）"),
        "__LAST_FRAME_LINE__": action.get("lastFrameLine", ""),
        "__PIPELINE_TAIL__": action.get("pipelineTail",
            "--min-interval-ms 30000 --max-interval-ms 60000"),
        "__RUNTIME_WIRING__": action.get("runtimeWiring",
            "----------------------------------------------------------------------"),
        "__ACTION_TYPE__": action.get("actionType", "偶发动作"),
        "__REVIEW_TONGUE_LINE__": action.get("reviewExtraTongue",
            "- 舌头/关键部位是否可见、节奏是否从容\n"),
        "__REVIEW_EYE_LINE__": action.get("reviewExtraEye",
            "- 眼睛状态是否符合本动作要求（见上方关键细节）\n"),
    }

    text = SKELETON.read_text(encoding="utf-8")
    unknown = {p for p in ALLOWED if p}  # 占位集合已固定，只校验未被替换的
    # envelopeRisk 里可能嵌了 __MARGIN_BOTTOM__，先填外层再填内层
    for _ in range(2):
        for k, v in values.items():
            text = text.replace(k, v)
    # 空占位符会留下孤立空行（如未配置 petExtra），压掉
    text = re.sub(r"\n[ \t]*\n(?:[ \t]*\n)+", "\n\n", text)

    left = {p for p in ALLOWED if p in text}
    if left:
        raise SystemExit(f"[硬失败] 骨架里仍有未替换的占位符: {sorted(left)}")
    return text


def main() -> int:
    ap = argparse.ArgumentParser(description="生成偶发动作 Seedance 提示词")
    ap.add_argument("--pet", help="petId（对应 output/宠物档案/<petId>.json）")
    ap.add_argument("--action", help="动作 id（对应 scripts/_提示词模板/<action>.json）")
    ap.add_argument("--serial", default="00", help="提示词编号（每只宠物各自的序号）")
    ap.add_argument("--out", help="输出文件路径")
    ap.add_argument("--list", action="store_true", help="列出可用档案与动作配置")
    # --all-pets：一次给所有宠物渲染同一份动作模板。
    # 为什么需要：提示词的维护单元只有 1 份（动作配置 + 骨架），
    # 但 Seedance 必须拿到填好猫特异变量的具体文本，所以物理上是 N 个文件。
    # 加这个参数是为了让「改一处 → 一条命令 → N 份同步」，不必逐个手敲。
    ap.add_argument("--all-pets", action="store_true",
                    help="给所有宠物生成同一动作（配合 --action/--serial/--subdir）")
    ap.add_argument("--subdir", default=None,
                    help="--all-pets 时的子目录，如 12-拎起；"
                         "默认 <serial>-<动作名>")
    args = ap.parse_args()

    if args.list:
        pets = sorted(p.name[:-5] for p in PROFILE_DIR.glob("*.json")) if PROFILE_DIR.exists() else []
        acts = sorted(p.stem for p in TEMPLATE_DIR.glob("*.json"))
        print("宠物档案:", pets or "(空)")
        print("动作配置:", acts or "(空)")
        return 0
    if not args.pet and not args.all_pets:
        ap.print_help()
        raise SystemExit("\n[失败] 必须指定 --pet 或 --all-pets")

    act_path = TEMPLATE_DIR / f"{args.action}.json"
    if not act_path.exists():
        raise SystemExit(f"[失败] 缺少动作配置: {act_path}")
    action = load_json(act_path)

    if args.all_pets:
        if args.out:
            raise SystemExit("[失败] --all-pets 与 --out 不能同时用（输出是多个文件）")
        pet_ids = sorted(p.stem for p in PROFILE_DIR.glob("*.json"))
        if not pet_ids:
            raise SystemExit(f"[失败] 宠物档案目录为空: {PROFILE_DIR}")
        subdir = args.subdir or f"{args.serial}-{action['actionName']}"
        print(f"[批量] 动作={action['actionId']}  宠物={len(pet_ids)} 只  "
              f"模板={act_path.name} + {SKELETON.name}\n")
        for pid in pet_ids:
            pet = load_json(PROFILE_DIR / f"{pid}.json")
            text = build(pet, action, args.serial)
            out = ROOT / pet["assetDir"] / subdir / "02-提示词" \
                / f"Seedance提示词-{action['actionId']}.txt"
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(text, encoding="utf-8")
            m = margins(pet["crop"])
            print(f"  [生成] {out}")
            print(f"         {pet['displayName']}  余量 上{m['top']} 下{m['bottom']} "
                  f"左{m['left']} 右{m['right']}")
        print(f"\n[完成] 同一份模板渲染 {len(pet_ids)} 份，"
              f"差异只有宠物档案里的变量（身份/毛色/取景/路径）")
        return 0

    pet_path = PROFILE_DIR / f"{args.pet}.json"
    for p in (pet_path,):
        if not p.exists():
            raise SystemExit(f"[失败] 缺少文件: {p}")

    pet = load_json(pet_path)
    text = build(pet, action, args.serial)

    out = Path(args.out) if args.out else (
        ROOT / pet["assetDir"] / "02-提示词"
        / f"Seedance提示词-{args.serial}-{action['actionName']}.txt")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")

    m = margins(pet["crop"])
    print(f"[生成] {out}")
    print(f"[宠物] {pet['displayName']}  取景 crop({pet['crop']['x']},"
          f"{pet['crop']['y']},{pet['crop']['size']})→{pet['crop']['outputSize']}")
    print(f"[余量] 上{m['top']} 下{m['bottom']} 左{m['left']} 右{m['right']} "
          f"({pet['crop']['sourceSize']} 空间)")
    print(f"[动作] {action['actionName']} ({action['actionId']}) "
          f"{action['duration']}s  骨架={SKELETON.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

# -*- coding: utf-8 -*-
"""偶发/交互动作（yawn / lick / grab-release）的**机器用**提示词。

## 与 idle-combo 那条（`frames/prompts.py`）的区别

idle-combo 的正文是**独立文件**（`loop-idle-combo.txt`），因为那份正文从头到尾
就是发给模型的话。偶发动作不一样：它住在**给人看的操作文档**里
（`action-skeleton.txt`，原本是 `scripts/_提示词模板/骨架.txt`），整份文档五节：

    【一、主提示词（直接复制粘贴）】   ← 发给模型
    【二、负向提示词 / 禁止项】        ← 发给模型
    【三、★ 取景框余量约束】          ← 给人看的（crop 数字、下游管线命令）
    【四、生成参数】                  ← 给人看的（模式/画幅/时长）
    【五、生成后第一件事】            ← 给人看的（复查清单）

所以这一层只做一件事：**按老脚本（`poc_生成绿幕视频.py::section`）同一套切分逻辑
把【一】【二】抽出来**，填值后拼成一份提示词。**不复制、不手抄正文** ——
骨架与动作配置都只有一份真源，改一处两边同步。

## 人读的那份文档还在

`scripts/poc_生成动作提示词.py` 仍要给老王产出**完整操作文档**（含下游管线命令、
生成参数、复查清单），用的是**同一份骨架**，不是两份。

## 服务侧只需要 4 个宠物字段

【一】【二】里因猫而异的只有 `identity` / `coat` / `coatGuard` / `coatNegative`。
它们原先写在 `output/宠物档案/<petId>.json`（**`output/` 被 gitignore，服务拿不到**，
而且只有 04/05/06 三份）。服务路径改由 `analyze_photo_facts` 从首帧图自动判定。
crop / 余量 / 锚点帧 / 路径全在【三】【四】【五】—— **服务一律不需要**。

## 拼法与 idle 保持一致

`lk888_client.submit_video` 只收**一个 `prompt` 串**（没有独立负向词字段），
所以负向词必须拼进去。拼法照 `frames/prompts.py` 的 `_NEGATIVE_PREFIX`：
`主提示词 + "\\n\\nAvoid the following: " + 负向词`。换一种拼法 = 换了一份提示词。

## 为什么这里要压空行

没配 `petExtra` 的宠物会留下一个孤立空行。老脚本用
`re.sub("\\n[ \\t]*\\n(?:[ \\t]*\\n)+", "\\n\\n", text)` 压掉 —— 这里必须用
**同一个正则**，否则两条路渲染出来的字节不一样（提示词差一个空行 = 换了份提示词）。
"""
from __future__ import annotations

import json
import re
from pathlib import Path

__all__ = [
    "ACTION_IDS",
    "NEGATIVE_PREFIX",
    "ActionPromptError",
    "joins_idle_schedule",
    "load_action",
    "render_action_prompt",
    "uses_end_frame",
]

_ASSETS = Path(__file__).resolve().parent.parent / "assets" / "motion-prompts"

SKELETON_FILE = "action-skeleton.txt"
ACTIONS_DIR = "actions"

# 允许的动作。**白名单**：动作 id 会变成 scratch 里的文件名与 manifest 里的
# `actionId`，不接受调用方临时发明一个。
ACTION_IDS = ("yawn", "lick", "grab-release")

# 【一】【二】两节里允许出现的占位符。其余（crop/余量/锚点帧/路径）都在人读的
# 三节里，服务拿不到也不该拿。多一个少一个都在这里报错 —— 渲染是纯字符串替换，
# 替换不到的占位符会**原样发给模型**，模型照着一行大括号去画。
_PROMPT_PLACEHOLDERS = frozenset({
    "__IDENTITY__", "__COAT__", "__COAT_GUARD__", "__COAT_NEGATIVE__",
    "__ACTION_SECTION__", "__ACTION_DETAILS__", "__ACTION_STRICT__",
    "__ACTION_NEGATIVE__", "__PET_EXTRA__", "__TAIL_HEADER__", "__TAIL_REQ__",
})

# 老脚本用 `set(stripped) == {"-"}` 认分隔行；这里的占位符形态是 `__NAME__`。
_PLACEHOLDER = re.compile(r"__[A-Z][A-Z0-9_]*__")

# 与 `poc_生成绿幕视频.py::_is_section_header` 逐字一致的判据。
_SECTION_NUM = re.compile(r"^[一二三四五六七八九十]+、")
_TOP_SECTION_PREFIXES = ("主提示词", "负向提示词", "背景", "为什么")

# 负向词的拼接方式与 `frames/prompts.py` 逐字一致。
NEGATIVE_PREFIX = "Avoid the following: "

# 老脚本压空行的同一个正则。
_BLANK_RUN = re.compile(r"\n[ \t]*\n(?:[ \t]*\n)+")

# 渲染时替换两轮：`tailReq` 之类的内容里可能还嵌了别的占位符（老脚本同做法）。
_REPLACE_ROUNDS = 2


class ActionPromptError(ValueError):
    """动作提示词资产缺失或参数越界。

    继承 `ValueError`（不是 `SystemExit`）：后者不是 `Exception`，
    `job_store.run_reserved` 的 `except Exception` 抓不到，会在 worker 线程里静默逃逸。
    """


def _is_section_header(line: str) -> bool:
    """顶层分节标题：`【` + 中文数字编号 + `、`，或以 主提示词/负向提示词/背景/为什么 开头。

    正文里的子标题（【动作安排…】【严格要求】等）不带编号也不是这些前缀 → False，
    不会被误判成分节边界。
    """
    stripped = line.strip()
    if not (stripped.startswith("【") and stripped.endswith("】")):
        return False
    inner = stripped[1:-1]
    return bool(_SECTION_NUM.match(inner)) or inner.startswith(_TOP_SECTION_PREFIXES)


def section(lines: list[str], header_keyword: str) -> str:
    """取 【xxx】 分节标题下、到下一个顶层分节标题或分隔行为止的正文。

    与 `scripts/poc_生成绿幕视频.py::section` **同一套规则**（那份是真源，
    5b 片就是这么切的 idle 提示词）。
    """
    start = None
    for index, line in enumerate(lines):
        if header_keyword in line and _is_section_header(line):
            start = index + 1
            break
    if start is None:
        return ""

    body: list[str] = []
    for line in lines[start:]:
        stripped = line.strip()
        if stripped and set(stripped) == {"-"}:  # 分隔行：出现在正文之后即结束
            if body:
                break
            continue
        if _is_section_header(line):
            break
        body.append(line)
    return "\n".join(body).strip()


def _skeleton_lines() -> list[str]:
    path = _ASSETS / SKELETON_FILE
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ActionPromptError(f"动作骨架读不到: {path}") from exc
    if not text.strip():
        raise ActionPromptError(f"动作骨架是空的: {path}")
    return text.splitlines()


def load_action(action_id: str) -> dict:
    """读一份动作配置（`actions/<id>.json`）。"""
    if action_id not in ACTION_IDS:
        raise ActionPromptError(f"不支持的动作: {action_id!r}")
    path = _ASSETS / ACTIONS_DIR / f"{action_id}.json"
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ActionPromptError(f"动作配置读不到: {path}") from exc
    try:
        action = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ActionPromptError(f"动作配置不是合法 JSON: {path}: {exc}") from exc
    if not isinstance(action, dict):
        raise ActionPromptError(f"动作配置必须是一个对象: {path}")
    if action.get("actionId") != action_id:
        raise ActionPromptError(
            f"动作配置的 actionId 与文件名不一致: {action.get('actionId')!r} != {action_id!r}"
        )
    for field in ("actionName", "section", "details", "strict", "negative", "duration"):
        if field not in action:
            raise ActionPromptError(f"动作配置缺少字段 {field}: {path}")
    return action


def uses_end_frame(action: dict) -> bool:
    """这个动作是不是「首尾帧」模式（同一张图传两次）。

    只有交互动作（`grab-release`）是：它必须**首尾回到同一张端坐图**，
    否则拎起来之后落不回原姿态。判据取自配置里的 `genMode` ——
    老脚本把它拼进人读文档的【四、生成参数】，服务这边只要这一个布尔。
    """
    return "尾帧" in str(action.get("genMode", ""))


def joins_idle_schedule(action: dict) -> bool:
    """这个动作进不进 `idleSchedule`（随机插入的偶发动作）。

    交互动作（`grab-release`）**不进**：它由 `playMotion` 触发（按下拖拽），
    进了 idleSchedule 就会自发放起来。判据取自配置里的 `pipelineTail`
    （老脚本给它的值是 `--no-idle-schedule`）。
    """
    return "--no-idle-schedule" not in str(action.get("pipelineTail", ""))


def _fill(text: str, values: dict[str, str], label: str) -> str:
    """填占位符 + 压空行 + 查残留。"""
    for _ in range(_REPLACE_ROUNDS):
        for key, value in values.items():
            text = text.replace(key, value)
    text = _BLANK_RUN.sub("\n\n", text).strip()
    left = sorted(set(_PLACEHOLDER.findall(text)))
    if left:
        raise ActionPromptError(f"{label}渲染后仍有未替换的占位符 {left}")
    return text


def render_action_prompt(
    action_id: str,
    *,
    identity: str,
    coat: str,
    coat_guard: str,
    coat_negative: str,
    pet_id: str | None = None,
) -> str:
    """渲染发给 Seedance 的动作提示词（主提示词 + 负向词）。

    四个宠物字段由 `analyze_photo_facts` 从首帧图自动判定；`pet_id` 只用于查
    动作配置里的 `petExtra`（给某只内置宠物的专属幅度约束），没有就是空串。
    """
    for label, value in (
        ("identity", identity),
        ("coat", coat),
        ("coat_guard", coat_guard),
        ("coat_negative", coat_negative),
    ):
        if not isinstance(value, str) or not value.strip():
            raise ActionPromptError(f"{label} 不能为空")

    action = load_action(action_id)
    pet_extra = str(action.get("petExtra", {}).get(pet_id, "")) if pet_id else ""
    duration = str(action["duration"])

    values = {
        "__IDENTITY__": identity,
        "__COAT__": coat,
        "__COAT_GUARD__": coat_guard,
        "__COAT_NEGATIVE__": coat_negative,
        "__ACTION_SECTION__": action["section"],
        "__ACTION_DETAILS__": action["details"],
        "__ACTION_STRICT__": action["strict"],
        "__ACTION_NEGATIVE__": action["negative"],
        # 没配 petExtra 时值是空串，所在行会被 `_fill` 的空行压缩去掉
        # （与老脚本同一条正则）。
        "__PET_EXTRA__": (pet_extra + "\n") if pet_extra else "",
        "__TAIL_HEADER__": action.get("tailHeader", "首尾一致性"),
        "__TAIL_REQ__": action.get(
            "tailReq",
            "第 0 秒和第 %s 秒，猫的端坐姿态、头部角度、耳朵角度、眼睛睁开程度必须高度一致\n"
            "（不强求像素级一致，因为是偶发动作而非循环视频）。\n"
            "下一段 idle-combo 循环视频的衔接由运行时 idleSchedule 处理。" % duration,
        ),
    }

    lines = _skeleton_lines()
    main = section(lines, "主提示词")
    if not main:
        raise ActionPromptError(f"骨架里找不到【主提示词】节: {_ASSETS / SKELETON_FILE}")
    negative = section(lines, "负向提示词")
    if not negative:
        raise ActionPromptError(f"骨架里找不到【负向提示词】节: {_ASSETS / SKELETON_FILE}")

    # 只对**要用的两节**查占位符白名单：人读的【三】【四】【五】里那些
    # crop/余量/路径占位符服务根本没有，也不该有。
    for label, body in (("主提示词", main), ("负向提示词", negative)):
        unknown = sorted(set(_PLACEHOLDER.findall(body)) - _PROMPT_PLACEHOLDERS)
        if unknown:
            raise ActionPromptError(f"{label}里有意料之外的占位符 {unknown}")

    return (
        f"{_fill(main, values, '主提示词')}\n\n"
        f"{NEGATIVE_PREFIX}{_fill(negative, values, '负向提示词')}"
    )

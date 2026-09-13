# -*- coding: utf-8 -*-
"""写实风（`frame-video-v1`）的**机器用提示词**。

## 为什么要有这一层

提示词是这个产品真正的「算法」：同样的照片，提示词变一个字，产出就换一只猫。
但在这之前它躺在两个**没进版本库**的地方：

- `output/_通用母版提示词-2026-09-12.txt`、`output/_通用组合循环提示词-2026-09-12.txt`
  —— `.gitignore` 里 `output/` 整个被忽略，服务**根本拿不到**这两份文件。
- 而且它们是**给人看的完整文档**：开头写着「与建国版的区别」、结尾写着「保存位置」，
  真正的提示词夹在中间，靠脚本按 `【主提示词】` / `-----` 切出来。人读的部分和
  机器用的部分混在一起，改注释就可能改到提示词。

所以这里把两者拆开：

    assets/motion-prompts/master.txt                     ← 机器用（含 {{SPECIES}}/{{COAT_LEN}}）
    assets/motion-prompts/loop-idle-combo.txt            ← 机器用（主提示词）
    assets/motion-prompts/loop-idle-combo.negative.txt   ← 机器用（负向词）
    assets/motion-prompts/README.md                      ← 人读的说明、参数、下游管线

正文**不是手抄的**：是从老文档里用老脚本自己的切分逻辑抽出来的，抽完逐字节比对过
（见落地清单第 5b 片）。改提示词就改这几个 `.txt`，改完跑 `frames/test_prompts.py`
—— 里面有 golden 哈希，提示词一动就会红，逼你意识到「这会改变所有新宠物的产出」。

## 为什么不把注释放进模板

`{{...}}` 之外的一切都会被原样发给模型。注释留在里面 = 注释也在花钱。
README.md 才是写注释的地方。
"""
from __future__ import annotations

import re
from pathlib import Path

__all__ = [
    "COAT_LENGTHS",
    "SPECIES_IDS",
    "PromptError",
    "render_loop_prompt",
    "render_master_prompt",
]

_ASSETS = Path(__file__).resolve().parent.parent / "assets" / "motion-prompts"

MASTER_TEMPLATE = "master.txt"
LOOP_TEMPLATE = "loop-idle-combo.txt"
LOOP_NEGATIVE_TEMPLATE = "loop-idle-combo.negative.txt"

SPECIES_IDS = ("cat", "dog")
# 毛长只影响母版提示词（视频那边的身份一律以首帧图为准）。
COAT_LENGTHS = {"short": "short-haired", "long": "long-haired"}

# 母版模板唯一允许出现的两个占位符。
_MASTER_PLACEHOLDERS = frozenset({"{{SPECIES}}", "{{COAT_LEN}}"})

# 只认「全大写标识符」形式的占位符：模板说明文字里的省略号不是占位符，
# 不能当残留报错。
_PLACEHOLDER = re.compile(r"\{\{[A-Z][A-Z0-9_]*\}\}")

# 负向词的拼接方式与 `scripts/poc_生成绿幕视频.py` 完全一致 ——
# 老链路就是这么发的，换一种写法等于换了一份提示词。
_NEGATIVE_PREFIX = "Avoid the following: "


class PromptError(ValueError):
    """提示词资产缺失或参数越界。

    继承 `ValueError` 而不是 `SystemExit`：后者不是 `Exception`，
    `job_store.run_reserved` 的 `except Exception` 抓不到，会在 worker 线程里静默逃逸。
    """


def _read(name: str, allowed: frozenset[str] = frozenset()) -> str:
    """读一份提示词资产。

    `allowed` 是这份模板**允许**存在的占位符。不在名单里的一律报错 ——
    模板里把 `{{SPECIES}}` 敲成 `{{SPECIE}}` 是不会报错的：渲染是纯字符串替换，
    替换不到就原样发给模型，模型照着一行大括号去画。这种错必须在这里拦住。

    注意：`{{` 只在渲染前的模板里有意义。所以校验放在这一层，**渲染后**由各
    `render_*` 再查一遍残留。

    读的是**文本**不是字节：`read_text` 会把 `\r\n` 归一成 `\n`，所以 golden 哈希
    不受 checkout 时 `core.autocrlf` 的影响（实测 CRLF / LF 磁盘上哈希相同）。
    改成 `read_bytes()` 就会把这个性质弄丢。
    """
    path = _ASSETS / name
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise PromptError(f"提示词资产读不到: {path}") from exc
    if not text.strip():
        raise PromptError(f"提示词资产是空的: {path}")
    unexpected = sorted(set(_PLACEHOLDER.findall(text)) - set(allowed))
    if unexpected:
        raise PromptError(f"提示词资产里有意料之外的占位符 {unexpected}: {path}")
    return text.strip()


def _require_no_placeholder(text: str, label: str) -> str:
    left = sorted(set(_PLACEHOLDER.findall(text)))
    if left:
        raise PromptError(f"{label}渲染后仍有未替换的占位符 {left}")
    return text


def render_master_prompt(species: str, coat: str | None = None) -> str:
    """母版提示词：照片 → 透明母版（`gpt-image-2`，约 0.06 算力）。

    只指定「物种 + 毛长档位」两个最外层约束，其余一律以照片为准 ——
    这是通用版相对建国版的关键改动（建国版把「银渐层」写死在提示词里）。

    **`coat=None` 表示「不知道毛长」，此时不提这一档**（服务路径就是这样：
    `FrameStepRequest` 里没有毛长字段，产品也问不出来）。
    少说这一句是安全的：提示词下一段本来就要求「毛长照照片一模一样」，
    毛长真正的来源是照片，`{{COAT_LEN}}` 只是一句提前的提示。
    """
    if species not in SPECIES_IDS:
        raise PromptError(f"不支持的物种: {species!r}")
    if coat is not None and coat not in COAT_LENGTHS:
        raise PromptError(f"不支持的毛长档位: {coat!r}")
    text = _read(MASTER_TEMPLATE, _MASTER_PLACEHOLDERS)
    rendered = (
        text.replace("{{COAT_LEN}}", COAT_LENGTHS[coat])
        if coat is not None
        # 连同后面的空格一起去掉，否则会留下 "this exact  cat." 这种双空格
        else text.replace("{{COAT_LEN}} ", "")
    )
    rendered = rendered.replace("{{SPECIES}}", species)
    return _require_no_placeholder(rendered, "母版提示词")


def render_loop_prompt(*, include_negative: bool = True) -> str:
    """组合循环提示词（呼吸 + 眨眼 + 摇尾焊死在一支视频里）。

    身份/毛色一律交给**首帧图**，所以正文里没有任何宠物相关的替换位 ——
    同一份提示词适配任意猫/狗，这也是「通用化」能达到的上限。

    `include_negative=False` 只用于 A/B 对比（保留老脚本的 `--no-negative`）。
    """
    main = _read(LOOP_TEMPLATE)
    if not include_negative:
        return main
    return f"{main}\n\n{_NEGATIVE_PREFIX}{_read(LOOP_NEGATIVE_TEMPLATE)}"

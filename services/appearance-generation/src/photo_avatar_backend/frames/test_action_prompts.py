# -*- coding: utf-8 -*-
"""动作提示词的契约测试。

## 为什么钉 golden 哈希

提示词是这个产品真正的「算法」：同样的照片，提示词变一个字，产出就换一只猫。
这几个哈希的作用不是「防止测试红」，而是**逼人意识到改动会改变所有新宠物的产出**
—— 骨架、动作配置、或者渲染逻辑动一下，这里就会红。

改哈希之前先问：这次改动是有意的吗？会影响已经在用的宠物吗？

## 这份 fixture 是写死的，不读 `output/宠物档案/`

`output/` 在 `.gitignore` 里 —— 测试读它就等于「在别的机器上必红」。
真档案（04/05/06）只用于**搬家时的一次性黄金回归**（见落地清单），不进测试。
"""
from __future__ import annotations

import hashlib

import pytest

from photo_avatar_backend.frames.action_prompts import (
    ACTION_IDS,
    ActionPromptError,
    joins_idle_schedule,
    load_action,
    render_action_prompt,
    uses_end_frame,
)

FACTS = {
    "identity": "短毛猫，圆脸、大而圆的眼睛、三角形耳朵、短而贴身的被毛、中等长度尾巴",
    "coat": "浅奶油白底毛（亮部 RGB≈(236,228,220)）+ 深色近黑斑纹与眼线（暗部 RGB≈(28,27,27)）",
    "coat_guard": "不得变灰、不得变蓝灰、不得发黄、不得整体提亮、不得失饱和、不得丢失深色斑纹",
    "coat_negative": "desaturation, grey, blue grey, yellowing, washed out, lost markings",
}

# 与 `render_action_prompt(..., **FACTS)` 的输出逐字节绑定。
GOLDEN = {
    "yawn": "2c4b3da92f72a10f71fa9908f4da107959e9a7221eb8ad0c37aea3a3436f236d",
    "lick": "f255c40f971b80cbf0d7540b00219b53ae49c7c22a41b1af9f06b284da1c617c",
    # 2026-09-15：grab-release 从「抬起约半个身位」改成「原地悬空、不许向上位移」。
    # 原因是实测必然出画：framing_ok 不校验上余量 + Seedance 重绘会把主体放大 ≈6%
    # ⇒ 上余量 7.5% 掉到 4.1%，抬 26px 头顶就顶边、连续 145 帧被画幅切平、耳朵消失。
    # 2026-09-19：改提示词也没管住 —— 三次实测「余量给得越大、它抬得越高」
    # （4.1%→80/145、11.9%→36/289、14.7%→126/289），模型是「一路抬到画幅上边」。
    # ⇒ 换语义：整套删掉「腾空/悬空/被托起/飘浮」，改成**原地姿态变化**
    #   —— 只有四条腿从收在身下改为自然下垂、尾巴垂落，位置与高度都不变。
    "grab-release": "1a59ab95a09aab4ab82e53176fb338330c791ee18c2aa95b796da581acac9015",
}


def render(action_id: str) -> str:
    return render_action_prompt(action_id, **FACTS)


@pytest.mark.parametrize("action_id", sorted(GOLDEN))
def test_rendered_prompt_matches_its_golden_hash(action_id: str) -> None:
    digest = hashlib.sha256(render(action_id).encode("utf-8")).hexdigest()
    assert digest == GOLDEN[action_id], (
        f"{action_id} 的提示词变了。"
        "如果这是有意的（改了骨架/动作配置/渲染逻辑），请同步更新 GOLDEN；"
        "但那意味着所有新宠物的产出都会变。"
    )


@pytest.mark.parametrize("action_id", sorted(GOLDEN))
def test_no_placeholder_survives_rendering(action_id: str) -> None:
    """替换不到的占位符会被**原样发给模型**，模型就照着一行大写标识符去画。"""
    import re

    text = render(action_id)
    left = sorted(set(re.findall(r"__[A-Z][A-Z0-9_]*__", text)))
    assert left == [], f"{action_id} 渲染后仍有占位符: {left}"


@pytest.mark.parametrize("action_id", sorted(GOLDEN))
def test_prompt_carries_the_pet_fields_and_the_shared_negative_prefix(action_id: str) -> None:
    text = render(action_id)
    # 四个宠物字段必须真的落进提示词（否则「渲染成功但没用上」也是坏的）。
    assert FACTS["identity"] in text
    assert FACTS["coat"] in text
    assert FACTS["coat_guard"] in text
    assert FACTS["coat_negative"] in text
    # 负向词的拼法与 idle 那条必须一致：换一种写法等于换了一份提示词。
    assert "\n\nAvoid the following: " in text
    # 绿幕与静止镜头是动作视频的硬前提（抠像靠它）。
    assert "绿幕" in text


def test_grab_release_never_asks_the_model_to_lift_the_cat() -> None:
    """三次实测：提示词里只要出现「腾空/悬空/被托起/飘浮」，模型就一路抬到画幅上边。

    这套语义是被实证否证过的（余量给越大抬越高），不允许再溜回提示词里。
    """
    text = render("grab-release")
    banned = ["腾空", "悬空", "被托起", "托起", "飘浮", "重力失效"]
    left = [word for word in banned if word in text]
    assert left == [], f"grab-release 提示词里又出现了被否证的提举语义: {left}"


def test_grab_release_is_the_only_end_frame_action() -> None:
    assert uses_end_frame(load_action("grab-release"))
    assert not uses_end_frame(load_action("yawn"))
    assert not uses_end_frame(load_action("lick"))


def test_grab_release_stays_out_of_the_idle_schedule() -> None:
    """交互动作由 playMotion 触发（按下拖拽），进 idleSchedule 就会自发放起来。"""
    assert not joins_idle_schedule(load_action("grab-release"))
    assert joins_idle_schedule(load_action("yawn"))
    assert joins_idle_schedule(load_action("lick"))


def test_unknown_action_is_rejected() -> None:
    with pytest.raises(ActionPromptError, match="不支持的动作"):
        render_action_prompt("sleep", **FACTS)


@pytest.mark.parametrize("field", sorted(FACTS))
def test_an_empty_pet_field_is_rejected(field: str) -> None:
    facts = dict(FACTS)
    facts[field] = "   "
    with pytest.raises(ActionPromptError, match=field):
        render_action_prompt("yawn", **facts)


def test_every_action_config_declares_its_own_id() -> None:
    """防止复制一份 json 忘了改 actionId —— 那样两个动作会渲染成同一份提示词。"""
    for action_id in ACTION_IDS:
        assert load_action(action_id)["actionId"] == action_id


def test_the_action_files_and_the_whitelist_agree() -> None:
    from photo_avatar_backend.frames import action_prompts

    directory = action_prompts._ASSETS / action_prompts.ACTIONS_DIR
    on_disk = sorted(p.stem for p in directory.glob("*.json"))
    assert on_disk == sorted(ACTION_IDS), (
        "assets/motion-prompts/actions 里的配置与 ACTION_IDS 白名单不一致 —— "
        "加了动作就要同时加白名单（它也是 scratch 文件名与 manifest 的 actionId）"
    )

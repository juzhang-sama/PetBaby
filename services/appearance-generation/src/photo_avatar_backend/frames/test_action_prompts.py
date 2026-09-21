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
    action_first_frame_master,
    action_frame_range,
    action_frame_target,
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
    # 2026-09-20 第二次改：换语义奏效（y0 min 0→82、触边帧 20/48→0/48），
    # 但暴露出**真正病根** —— 脚本写「约 5 秒」而产线锁死 VIDEO_DURATION="12"
    # ⇒ 模型被迫自己填满多出来的 7 秒，于是「转向侧身 + 把画幅横向撑满到 604」
    #   （crop box 只到 609，左右各裁掉一截）。
    # ⇒ 本版把时间轴**按 12 秒重写**，让「保持段」占全片四分之三以上（1.0-10.2 秒），
    #   并把 section/details/strict 里的中文否定句（「不转身、不侧身」）**全部改成正向描述** ——
    #   09-19 的教训是中文否定句等于把那个动作名喂给模型，禁止项只留给英文 negative。
    # 2026-09-20 晚（第三次改，本版）：老王拿短毛猫实测反馈「**前腿不变、后腿劈叉**」——
    #   原地语义把**动作本身也治没了**（身材比例清楚的短毛猫一眼就看出来）。
    #   回到建国那套（原文 = `output/宠物动作-建国-v1-2026-09-03/12-拎起/02-提示词/
    #   Seedance提示词-grab-release.txt`）：**明说腾空**
    #   （`被看不见的力量轻轻托起`、`四只腿自然垂落、尾巴向下垂`）
    #   **+ 身体钉成折叠态**（`从头顶点到最低一只脚掌的总高度与首帧端坐时基本相等`）。
    #   09-15 那次之所以顶边，缺的正是后半条「折叠」约束 ⇒ 身体一路往上飘。
    #   ⚠️ 顶边风险随之回归（暹罗首帧上余量实测只有 6.5%）⇒ **重出后必须量逐帧头顶余量再进包**。
    # 2026-09-20 深夜（第四次改，本版）：**正文一字未改，只把两句说反了的话改对。**
    #   真正的病根在代码不在文案：`_generate_video(end_frame=True)` 是 `images = [frame, frame]`
    #   （同一张传两次）；首帧换成拎起母版（背弓悬垂）之后，尾帧也成了悬垂
    #   ⇒ 模型被要求「结尾回到悬垂」⇒ 落回永远坐不正（实测末帧 vs idle 锚点 IoU 0.5477）
    #   ⇒ 松手切 idle 是一个明显跳变（老王：「落地没有站稳就瞬间变回」）。
    #   本版改对的是：`section` 的「首帧与尾帧都是同一张端坐图」+ `tailReq` 的
    #   「首帧与尾帧是同一张图」；代码侧尾帧改传 **idle 的端坐绿幕首帧**
    #   （守卫测试 `test_frame_pipeline.py::
    #   test_the_lift_tail_frame_is_the_seated_frame_not_its_own_first_frame`）。
    "grab-release": "c128e8630676066cb036f4d3729dd1e898d665f5edc4d0f35e4a6786491e5d14",
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


def test_grab_release_asks_for_a_folded_lift_not_a_pose_change() -> None:
    """锁「腾空语义 + 折叠约束」这对组合 —— 少任何一半都会坏。

    - 少了**腾空**：模型只敢动局部肢体 ⇒ 前腿不变、后腿劈叉
      （老王 09-20 拿身材比例清楚的短毛猫实测反馈）。
    - 少了**折叠**：身体一路往上飘 ⇒ 顶到画幅上边（09-15 / 09-18 / 09-19 四次实测）。

    建国（内置 05）两个都有（原文 = `output/宠物动作-建国-v1-2026-09-03/12-拎起/02-提示词/
    Seedance提示词-grab-release.txt`）。它多一道保险：先做一张**背弓悬垂的拎起母版**当首帧，
    视频里再用「总高度不超过首帧坐姿」把身体钉住 —— 腾出的空间留给下垂的腿，头顶不用上移。
    本产线只有一张坐姿首帧，所以更依赖提示词里那条折叠约束。
    """
    text = render("grab-release")
    # ① 腾空 + 悬垂：没有它，四条腿不会真的垂下来（前腿不动、后腿劈叉）
    assert "腾空" in text, "grab-release 又退回「原地姿态变化」了：四条腿不会垂下来"
    assert "自然垂落" in text or "悬垂" in text
    # ② 折叠：没有它，身体会一路抬到画幅上边（09-15~09-19 实测）
    assert "自然弯曲" in text, "缺「身体保持弯曲」⇒ 身体会被拉直、整体高度超过首帧"
    assert "总高度" in text, "缺「总高度与首帧端坐时基本相等」⇒ 顶边风险回归"
    # ③ 无外力源：建国原文的硬要求（画面里不许出现手/夹子/绳子）
    assert "没有任何可见的抓取工具" in text
    # ④ 负向词里**不许**再出现禁止抬升的那几条 —— 它们与「腾空」自相矛盾
    for word in ("rising", "floating upward", "moving up", "drifting upward"):
        assert word not in text, f"负向词里的 {word!r} 会直接禁掉要的抬升动作"


def test_grab_release_is_the_only_end_frame_action() -> None:
    assert uses_end_frame(load_action("grab-release"))
    assert not uses_end_frame(load_action("yawn"))
    assert not uses_end_frame(load_action("lick"))


# ---- 动作专用首帧（2026-09-20：拎起必须有自己的首帧）----


def test_only_the_lift_uses_its_own_first_frame() -> None:
    """**首帧定义了姿态起点**：只有拎起需要一张自己的母版首帧。

    别的动作必须继续与 idle **共用同一张首帧** —— 那是「触发动作时不跳变」的前提。
    这条一旦松开（比如给 yawn 也配一个 `firstFrameMaster`），产线会给每支动作各花
    0.06 算力，而且各支的身份锚开始互相漂。
    """
    assert action_first_frame_master(load_action("grab-release")) == "master-lift.txt"
    for action_id in ACTION_IDS:
        if action_id != "grab-release":
            assert action_first_frame_master(load_action(action_id)) is None


def test_the_lift_master_prompt_asset_exists_and_renders() -> None:
    """`master-lift.txt` 必须真的存在，且物种/毛长两个待填位置都填得上。

    它在配置里是**可选键**，`load_action` 不检查 ⇒ 少了这份资产只有真跑才炸，
    而一次真跑是 2.84 算力。所以在这里提前钉住。
    """
    from photo_avatar_backend.frames.prompts import render_master_prompt

    text = render_master_prompt("cat", "short", template="master-lift.txt")
    assert text.startswith("Use the uploaded seated character master image")
    assert "short-haired" in text
    # 折叠约束就是它存在的理由：少了它模型会一路往上飘（09-15~09-19 四次实测）
    assert "ALL FOUR PAWS ARE RAISED OFF THE GROUND" in text
    assert "must be roughly equal to" in text
    # 毛长写死就废了（猫/狗、长毛/短毛要共用同一份），`coat=None` 时必须能渲染
    assert render_master_prompt("dog", None, template="master-lift.txt").startswith(
        "Use the uploaded seated character master image"
    )
    assert "long-haired" not in render_master_prompt("dog", None, template="master-lift.txt")


@pytest.mark.parametrize("raw", ["../secret.txt", "sub/master.txt", "", 5, "master.txt.bak"])
def test_a_bad_first_frame_master_is_rejected(raw: object) -> None:
    """它只认资产目录下的**文件名**：放过一个 `/` 就指到资产目录外面去了。"""
    with pytest.raises(ActionPromptError, match="firstFrameMaster"):
        action_first_frame_master({"firstFrameMaster": raw})


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


# ---- 精修参数（都是可选的，没有就不裁不压）----


def test_refinement_keys_are_absent_by_default() -> None:
    assert action_frame_range({"actionId": "yawn"}) is None
    assert action_frame_target({"actionId": "yawn"}) is None


# 精修预算 = **从内置宠量出来的**（帧数 × 帧时长 = 单次时长）。改它等于改手感。
# 建国（内置 05）：grab-release 81 帧 3.40s、yawn 119 帧 5.00s、lick 119 帧 5.00s。
REFINEMENT_BUDGET = {
    "grab-release": ((0, 236), 81),
    "yawn": ((0, 227), 119),
    "lick": ((0, 249), 119),
}


@pytest.mark.parametrize("action_id", sorted(REFINEMENT_BUDGET))
def test_every_one_shot_action_declares_its_refinement_budget(action_id: str) -> None:
    """精修数字是**对标内置 05 建国量出来的**，改它等于改手感。"""
    expected_range, expected_target = REFINEMENT_BUDGET[action_id]
    action = load_action(action_id)

    assert action_frame_range(action) == expected_range
    assert action_frame_target(action) == expected_target


def test_only_one_shot_actions_are_refined() -> None:
    """idle-combo 是循环支：裁区间 / 重采样会改循环长度 ⇒ 它不该出现在精修表里。"""
    assert "idle-combo" not in REFINEMENT_BUDGET


@pytest.mark.parametrize("raw", [[0], [0, 1, 2], "0-1", [1, 0], [-1, 5], [True, 5], [0.5, 5]])
def test_a_malformed_frame_range_is_rejected(raw: object) -> None:
    with pytest.raises(ActionPromptError, match="frameRange"):
        action_frame_range({"frameRange": raw})


@pytest.mark.parametrize("raw", [0, 1, -3, True, "81", 2.5])
def test_a_malformed_frame_target_is_rejected(raw: object) -> None:
    with pytest.raises(ActionPromptError, match="frameTarget"):
        action_frame_target({"frameTarget": raw})

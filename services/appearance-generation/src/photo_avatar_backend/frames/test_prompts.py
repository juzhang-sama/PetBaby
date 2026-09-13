"""写实风提示词测试。

**这里的 sha256 是提示词合同，不是实现细节。** 提示词是这个产品真正的算法：
改一个字，所有新宠物的产出都会变。哈希红了不是「更新一下哈希」，
是要意识到自己在改产品行为 —— 改完把新值同步回
`assets/motion-prompts/README.md` 那张表。

（那 6 个哈希同时也是「与 2026-09-12 那版通用提示词逐字节相同」的证明：
搬家没重写。）
"""

from __future__ import annotations

import hashlib

import pytest

from photo_avatar_backend.frames.prompts import (
    COAT_LENGTHS,
    SPECIES_IDS,
    PromptError,
    render_loop_prompt,
    render_master_prompt,
)

GOLDEN = {
    ("cat", "short"): "80f687a73b99f61c4699e1ed0ced43dd183e7acfd7921a01bc43bb3882e60300",
    ("cat", "long"): "4deac451c9737d98ec3901bae107d97e37e206ae5983290285e9adf0ea1b237c",
    ("dog", "short"): "5250322d657870845e1991a4910e6229225d318dc25d790893a8876dd03da675",
    ("dog", "long"): "6f3dbd5a6feaf91416c55eb704eb7c530a46dc3f2a6301a768b36f7dae4ee672",
}
# 服务路径：`FrameStepRequest` 里没有毛长字段，走的是「不提这一档」的渲染。
GOLDEN_WITHOUT_COAT = {
    "cat": "968ecf75058864d3925ea716ca6b15d136f805d96d06a71bc0067556bd3e6935",
    "dog": "26767615710f07fb8bd2d18973e22ea2a56510f8b99764cead870744f0d3c6f1",
}
GOLDEN_LOOP = "51d666c398be47fa62d618527a90ea3a444f0a7f45b49f1bf64a870ba418f4f9"
GOLDEN_LOOP_NO_NEGATIVE = (
    "65d8d23efe349e71c552f56cd6ebb6c094d3308264dbde1312c19d27c405193c"
)


def _sha256(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


@pytest.mark.parametrize(("species", "coat"), sorted(GOLDEN))
def test_master_prompt_matches_the_shipped_contract(species: str, coat: str):
    assert _sha256(render_master_prompt(species, coat)) == GOLDEN[(species, coat)]


@pytest.mark.parametrize("species", sorted(GOLDEN_WITHOUT_COAT))
def test_master_prompt_without_a_coat_hint_matches_the_shipped_contract(species: str):
    """服务路径不带毛长档位（请求里没有这个字段）。"""
    assert _sha256(render_master_prompt(species)) == GOLDEN_WITHOUT_COAT[species]


@pytest.mark.parametrize("species", sorted(GOLDEN_WITHOUT_COAT))
def test_omitting_the_coat_hint_leaves_no_seam(species: str):
    """去掉整个档位（含尾随空格），不能留下 `this exact  cat.` 这种双空格。"""
    prompt = render_master_prompt(species)
    assert f"this exact {species}." in prompt
    assert "  " not in prompt
    assert "{{" not in prompt


def test_the_coat_hint_is_the_only_difference_from_the_hintless_prompt():
    for species in sorted(GOLDEN_WITHOUT_COAT):
        for coat, text in COAT_LENGTHS.items():
            with_hint = render_master_prompt(species, coat)
            without = render_master_prompt(species)
            assert with_hint.replace(f"{text} {species}", species) == without


@pytest.mark.parametrize(("species", "coat"), sorted(GOLDEN))
def test_master_prompt_states_the_species_and_coat_length(species: str, coat: str):
    """哈希保证「没变」，这条保证「变了的时候错在哪看得见」。"""
    prompt = render_master_prompt(species, coat)
    assert f"this exact {COAT_LENGTHS[coat]} {species}" in prompt
    assert "{{" not in prompt


def test_the_master_prompt_differs_only_by_species_and_coat():
    """归一化后必须**完全相同** —— 多出来的差异说明模板里混进了别的东西。

    归一化的锚点是那句身份声明 `…for this exact [<毛长> ]<物种>.`：
    带毛长档位时它长一点，不带的短一点，把两处都换成哑元就该处处相同。
    """
    cases = list(GOLDEN) + [(species, None) for species in GOLDEN_WITHOUT_COAT]
    normalized = set()
    for species, coat in cases:
        prompt = render_master_prompt(species, coat)
        head = f"this exact {COAT_LENGTHS[coat]} {species}." if coat else (
            f"this exact {species}."
        )
        assert head in prompt, head
        normalized.add(prompt.replace(head, "this exact <COAT> <SPECIES>."))
    assert len(normalized) == 1
    # 归一站得住的前提：物种/毛长各只出现一次，否则替换会波及正文
    for species, coat in cases:
        prompt = render_master_prompt(species, coat)
        assert prompt.count(species) == 1
        if coat is not None:
            assert prompt.count(COAT_LENGTHS[coat]) == 1


def test_loop_prompt_matches_the_shipped_contract():
    assert _sha256(render_loop_prompt()) == GOLDEN_LOOP
    assert _sha256(render_loop_prompt(include_negative=False)) == GOLDEN_LOOP_NO_NEGATIVE


def test_loop_prompt_appends_the_negative_verbatim():
    """负向词的拼接方式是老链路定死的（`Avoid the following: `），别改措辞。"""
    prompt = render_loop_prompt()
    main = render_loop_prompt(include_negative=False)
    assert prompt.startswith(main + "\n\nAvoid the following: ")
    assert prompt != main


def test_loop_prompt_carries_no_pet_specific_text():
    """身份/毛色一律交给首帧图 —— 正文里不该出现物种或毛长档位。

    这是「通用化」的边界：一旦提示词里写死了「猫」，狗就用不了。
    """
    prompt = render_loop_prompt()
    assert "{{" not in prompt
    assert "银渐层" not in prompt
    for coat in COAT_LENGTHS.values():
        assert coat not in prompt
    # 「猫/狗」是以「任意猫/狗」这种泛称出现的，不许出现物种限定词
    assert "这只猫" not in prompt


@pytest.mark.parametrize("species", ["", "CAT", "rabbit", "cat ", None, 1])
def test_master_prompt_rejects_an_unknown_species(species: object):
    with pytest.raises(PromptError, match="物种"):
        render_master_prompt(species, "short")  # type: ignore[arg-type]


@pytest.mark.parametrize("coat", ["", "SHORT", "hairless", "short ", "Short"])
def test_master_prompt_rejects_an_unknown_coat(coat: object):
    with pytest.raises(PromptError, match="毛长档位"):
        render_master_prompt("cat", coat)  # type: ignore[arg-type]


def test_omitting_the_coat_is_allowed_but_not_silently_defaulted():
    """`None` = 不提这一档，不是「默认短毛」—— 长毛猫被写成 short-haired 更糟。"""
    prompt = render_master_prompt("cat", None)
    assert "short-haired" not in prompt and "long-haired" not in prompt
    assert prompt != render_master_prompt("cat", "short")


def test_supported_values_are_the_contract_ones():
    assert SPECIES_IDS == ("cat", "dog")
    assert tuple(COAT_LENGTHS) == ("short", "long")

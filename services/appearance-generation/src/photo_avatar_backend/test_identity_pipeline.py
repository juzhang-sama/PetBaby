import copy
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from photo_avatar_backend.contracts import (  # noqa: E402
    ContractError,
    SourceImage,
    StepRequest,
)
from photo_avatar_backend.pipelines import (  # noqa: E402
    analyze_identity,
    complete_appearance,
    validate_completion,
    validate_profile,
)


TRAIT_KEYS = (
    "faceShape",
    "faceProportions",
    "furColors",
    "markings",
    "eyeShape",
    "eyeColor",
    "earShape",
    "bodyType",
    "tail",
    "signatureMarks",
    "temperament",
)


class FakeLk888Client:
    def __init__(self, response):
        self.response = response
        self.calls = []

    def analyze_json(self, prompt, images, schema):
        self.calls.append((prompt, list(images), schema))
        return copy.deepcopy(self.response)


def trait(key, value="observed", source="user", evidence=None):
    return {
        "key": key,
        "value": value,
        "source": source,
        "evidencePhotoIds": ["photo-front"] if evidence is None and source == "user" else evidence or [],
    }


def profile_with_observed_traits():
    return {
        "schemaVersion": 1,
        "species": "cat",
        "style": "animated-film-soft-v1",
        "bodyModuleId": "body-balanced-v1",
        "bodyModuleSource": "user",
        "traits": [trait("faceShape", "round"), trait("bodyType", "balanced")],
        "completionSummary": [],
    }


def request(step, profile=None, locked_traits=(), source_images=()):
    return StepRequest(
        session_id="session-1",
        revision=0,
        provider_session_id=None if step == "analyzeIdentity" else "provider-1",
        step=step,
        attempt=1,
        consent_version="consent-v1",
        source_images=tuple(source_images),
        profile=profile,
        body_module_contract_sha256=None,
        modification=None,
        locked_traits=tuple(locked_traits),
    )


def missing_keys(profile):
    seen = {entry["key"] for entry in profile["traits"]}
    return [key for key in TRAIT_KEYS if key not in seen]


def valid_completion(profile):
    missing = missing_keys(profile)
    return {
        "requestedTraitKeys": missing,
        "completedTraits": [trait(key, "ai inferred", "ai-completed", []) for key in missing],
        "bodyModuleId": profile["bodyModuleId"],
        "bodyModuleSource": profile["bodyModuleSource"],
    }


def test_user_traits_require_photo_evidence_and_ai_traits_require_summary():
    without_evidence = profile_with_observed_traits()
    without_evidence["traits"][0]["evidencePhotoIds"] = []
    with pytest.raises(ContractError, match="evidencePhotoIds"):
        validate_profile(without_evidence)

    missing_summary = profile_with_observed_traits()
    missing_summary["traits"][0]["source"] = "ai-completed"
    missing_summary["traits"][0]["evidencePhotoIds"] = []
    with pytest.raises(ContractError, match="completionSummary"):
        validate_profile(missing_summary)


def test_completion_cannot_change_observed_or_locked_traits():
    before = profile_with_observed_traits()
    changed = copy.deepcopy(before)
    changed["traits"][0]["value"] = "triangular"
    with pytest.raises(ContractError, match="locked trait changed: faceShape"):
        validate_completion(before, changed, ["faceShape"])


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (lambda value: value["traits"].append(trait("unknown")), "unsupported trait key"),
        (lambda value: value.update(species="dog"), "species must be cat"),
        (lambda value: value.update(style="template-v1"), "style must be animated-film-soft-v1"),
        (lambda value: value["traits"].append(trait("faceShape", "other")), "duplicate trait key"),
        (lambda value: value["traits"][0].update(source="template"), "source"),
        (lambda value: value["traits"][0].update(value="x" * 513), "value is too long"),
    ],
)
def test_validate_profile_fails_closed_for_untrusted_trait_values(mutate, message):
    profile = profile_with_observed_traits()
    mutate(profile)
    with pytest.raises(ContractError, match=message):
        validate_profile(profile)


def test_validate_profile_rejects_unknown_fields_and_normalizes_strings():
    profile = profile_with_observed_traits()
    profile["traits"][0]["value"] = "  round  "
    profile["extra"] = True
    with pytest.raises(ContractError, match="unknown profile field: extra"):
        validate_profile(profile)

    profile.pop("extra")
    assert validate_profile(profile)["traits"][0]["value"] == "round"


def test_validate_completion_locks_value_source_and_evidence_and_body_module():
    before = profile_with_observed_traits()
    for field, value in (("source", "ai-completed"), ("evidencePhotoIds", ["photo-side"])):
        changed = copy.deepcopy(before)
        changed["traits"][0][field] = value
        if field == "source":
            changed["traits"][0]["evidencePhotoIds"] = []
            changed["completionSummary"] = ["faceShape"]
        with pytest.raises(ContractError, match="locked trait changed: faceShape"):
            validate_completion(before, changed, ["faceShape"])

    changed = copy.deepcopy(before)
    changed["bodyModuleId"] = "body-rounded-v1"
    with pytest.raises(ContractError, match="locked body module changed"):
        validate_completion(before, changed, ["bodyType"])


def test_analyze_identity_uses_only_injected_client_and_forbids_completion_prompting():
    source = SourceImage("photo-front", b"fixture-image", "0" * 64, 256, 256)
    client = FakeLk888Client(profile_with_observed_traits())

    result = analyze_identity(
        request("analyzeIdentity", source_images=[source]), client=client
    )

    assert result["traits"][0]["source"] == "user"
    prompt, images, schema = client.calls[0]
    assert images == [b"fixture-image"]
    assert "Do not infer" in prompt
    assert "Use source=user" in prompt
    assert schema["additionalProperties"] is False


def test_analyze_identity_rejects_an_ai_completed_or_template_profile():
    response = profile_with_observed_traits()
    response["traits"][0]["source"] = "ai-completed"
    response["traits"][0]["evidencePhotoIds"] = []
    response["completionSummary"] = ["faceShape"]
    with pytest.raises(ContractError, match="analysis may only return user traits"):
        analyze_identity(request("analyzeIdentity"), client=FakeLk888Client(response))


def test_analyze_identity_defers_body_module_when_photos_lack_body_evidence():
    response = profile_with_observed_traits()
    response["traits"] = [response["traits"][0]]
    response["bodyModuleSource"] = "ai-completed"
    client = FakeLk888Client(response)

    result = analyze_identity(request("analyzeIdentity"), client=client)

    assert result["bodyModuleSource"] == "ai-completed"
    assert all(entry["key"] != "bodyType" for entry in result["traits"])
    prompt, _, schema = client.calls[0]
    assert "defer the body module" in prompt
    assert schema["properties"]["bodyModuleSource"]["enum"] == [
        "user",
        "ai-completed",
    ]


def test_complete_appearance_fills_only_missing_traits_and_preserves_observed_body_module():
    before = profile_with_observed_traits()
    client = FakeLk888Client(valid_completion(before))

    result = complete_appearance(request("completeAppearance", before), client=client)

    assert [entry["key"] for entry in result["traits"]] == list(TRAIT_KEYS)
    assert result["traits"][0] == trait("faceShape", "round")
    assert result["bodyModuleId"] == "body-balanced-v1"
    assert result["completionSummary"] == sorted(missing_keys(before))
    prompt, images, schema = client.calls[0]
    assert images == []
    assert "only missing traits" in prompt
    assert "bodyModuleId" in prompt
    assert schema["additionalProperties"] is False
    assert "uniqueItems" not in json.dumps(schema, sort_keys=True)
    completed_trait_schema = schema["properties"]["completedTraits"]["items"]
    assert completed_trait_schema["properties"]["evidencePhotoIds"]["items"] == {
        "type": "string"
    }


def test_complete_prompt_embeds_deterministic_parseable_profile_json():
    before = profile_with_observed_traits()
    before["traits"][0]["value"] = "奶油猫咪's round"
    client = FakeLk888Client(valid_completion(before))

    complete_appearance(request("completeAppearance", before), client=client)

    prompt, _, _ = client.calls[0]
    start = prompt.index("<PROFILE_JSON>") + len("<PROFILE_JSON>")
    end = prompt.index("</PROFILE_JSON>", start)
    encoded = prompt[start:end]
    expected = json.dumps(
        before,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    assert encoded == expected
    assert json.loads(encoded) == before
    assert "奶油猫咪" in encoded


def test_complete_appearance_rejects_species_rewrite_duplicate_traits_and_body_module_overreach():
    before = profile_with_observed_traits()
    for mutate, message in (
        (lambda value: value.update(species="cat"), "unknown completion field: species"),
        (
            lambda value: value["completedTraits"].append(value["completedTraits"][0]),
            "duplicate completed trait key",
        ),
        (
            lambda value: value["requestedTraitKeys"].append(
                value["requestedTraitKeys"][0]
            ),
            "duplicate requested trait key",
        ),
        (lambda value: value.update(bodyModuleId="body-rounded-v1"), "bodyModuleId"),
    ):
        response = valid_completion(before)
        mutate(response)
        with pytest.raises(ContractError, match=message):
            complete_appearance(
                request("completeAppearance", before), client=FakeLk888Client(response)
            )

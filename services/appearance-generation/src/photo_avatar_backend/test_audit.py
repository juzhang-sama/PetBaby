from __future__ import annotations

from dataclasses import replace

import pytest

from .audit import (
    API_CONTRACT_VERSION,
    AttemptAuditV1,
    AuditContractError,
)


def _audit() -> AttemptAuditV1:
    return AttemptAuditV1(
        session_id="desktop-session-1",
        revision=7,
        attempt=1,
        provider_task_id="lk888-task-1",
        provider_model="gpt-image-2",
        provider_raw_sha256="1" * 64,
        canonical_sha256="2" * 64,
        body_module_id="body-balanced-v1",
        module_contract_sha256="3" * 64,
        source_texture_sha256="4" * 64,
        source_alpha_sha256="5" * 64,
        work_canvas_sha256="6" * 64,
        region_map_sha256="7" * 64,
        composer_version="deterministic-alpha-v1",
        png_encoder_version="pillow-png-v1",
        coverage_report={"minimumChangeRatio": 0.95},
        status="succeeded",
        error_code=None,
        created_at="2026-08-17T00:00:00+00:00",
        completed_at="2026-08-17T00:00:01+00:00",
    )


def test_attempt_audit_round_trips_strict_safe_wire():
    wire = _audit().to_wire()

    assert wire["apiContractVersion"] == API_CONTRACT_VERSION
    assert wire["provider"] == "lk888"
    assert wire["modelDisplayName"] == "GPT-image-2.0"
    assert AttemptAuditV1.from_wire(wire) == _audit()
    assert not ({"pngBase64", "prompt", "url", "apiKey"} & set(wire))


@pytest.mark.parametrize(
    "mutation",
    (
        lambda wire: wire.update(unexpected="rejected"),
        lambda wire: wire.update(canonicalSha256="A" * 64),
        lambda wire: wire.update(provider="other"),
        lambda wire: wire.update(status="running"),
    ),
)
def test_attempt_audit_rejects_unknown_or_noncanonical_wire(mutation):
    wire = _audit().to_wire()
    mutation(wire)

    with pytest.raises(AuditContractError):
        AttemptAuditV1.from_wire(wire)


def test_failed_audit_requires_error_and_has_no_canonical_output():
    failed = replace(
        _audit(),
        provider_task_id=None,
        provider_raw_sha256=None,
        canonical_sha256=None,
        status="failed",
        error_code="temporaryUnavailable",
    )

    assert AttemptAuditV1.from_wire(failed.to_wire()) == failed

from __future__ import annotations

import json
from pathlib import Path
import subprocess
from typing import Final

import pytest

from .contracts import ContractError, PixelAppearanceProfile
from .pixel_audit import PixelAuditError, parse_pixel_avatar_audit


REPO_ROOT: Final = Path(__file__).resolve().parents[4]
FIXTURE_PATH: Final = (
    REPO_ROOT
    / "apps"
    / "desktop"
    / "src-tauri"
    / "tests"
    / "fixtures"
    / "photo-avatar"
    / "pixel-style-v1-v2.json"
)
RUST_CRATE: Final = REPO_ROOT / "apps" / "desktop" / "src-tauri"


def test_python_parses_shared_v1_v2_profiles_and_audits() -> None:
    fixture = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))

    legacy_profile = PixelAppearanceProfile.parse(fixture["legacyProfile"])
    current_profile = PixelAppearanceProfile.parse(fixture["currentProfile"])
    legacy_audit = parse_pixel_avatar_audit(fixture["legacyAudit"])
    current_audit = parse_pixel_avatar_audit(fixture["currentAudit"])

    assert legacy_profile.style_profile_id == "pixel-style-v1"
    assert current_profile.style_profile_id == "pixel-style-v2-animation-ready"
    assert legacy_audit.schema_version == 1
    assert current_audit.schema_version == 2

    fixture["currentAudit"]["styleProfileSha256"] = fixture["legacyAudit"][
        "styleProfileSha256"
    ]
    with pytest.raises(PixelAuditError, match="fixed metadata"):
        parse_pixel_avatar_audit(fixture["currentAudit"])

    fixture["legacyAudit"]["styleProfileSha256"] = (
        "2a48f382d0d0a579010ffae2ce90a7693d364a0cf64e5463e0ce7bf0291ee4ab"
    )
    with pytest.raises(PixelAuditError, match="fixed metadata"):
        parse_pixel_avatar_audit(fixture["legacyAudit"])

    fixture["currentProfile"]["styleProfileId"] = "pixel-style-v3"
    with pytest.raises(ContractError, match="styleProfileId"):
        PixelAppearanceProfile.parse(fixture["currentProfile"])


def test_rust_consumes_the_same_pixel_contract_fixture() -> None:
    result = subprocess.run(
        [
            "cargo",
            "test",
            "shared_pixel_contract_fixture_preserves_v1_and_v2",
            "--lib",
        ],
        cwd=RUST_CRATE,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "shared_pixel_contract_fixture_preserves_v1_and_v2 ... ok" in result.stdout

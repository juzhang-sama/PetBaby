import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from photo_avatar_backend.config import BackendConfig, ConfigError  # noqa: E402


def valid_env(**overrides: str) -> dict[str, str]:
    env = {
        "LK888_API_KEY": "provider-secret",
        "PHOTO_AVATAR_BACKEND_TOKEN": "desktop-only-token",
    }
    env.update(overrides)
    return env


def test_config_requires_separate_backend_and_lk888_tokens():
    with pytest.raises(ConfigError, match="PHOTO_AVATAR_BACKEND_TOKEN"):
        BackendConfig.from_env({"LK888_API_KEY": "provider-secret"})

    with pytest.raises(ConfigError, match="LK888_API_KEY"):
        BackendConfig.from_env({"PHOTO_AVATAR_BACKEND_TOKEN": "desktop-only-token"})


def test_config_uses_frozen_models_and_resolved_state_directory(tmp_path):
    config = BackendConfig.from_env(
        valid_env(PHOTO_AVATAR_BACKEND_STATE_DIR=str(tmp_path / "state"))
    )

    assert config.lk888_base_url == "https://api.lk888.ai"
    assert config.analysis_model == "gpt-4o"
    assert config.image_model == "gpt-image-2"
    assert config.provider == "lk888"
    assert config.model_display_name == "GPT-image-2.0"
    assert config.api_contract_version == "lk888-media-generate-v1"
    assert config.privacy_policy_version == "unverified"
    assert config.retention_policy == "unverified"
    assert config.upstream_delete_api == "unsupported"
    assert config.host == "127.0.0.1"
    assert config.port == 8787
    assert config.state_dir == (tmp_path / "state").resolve()


@pytest.mark.parametrize(
    "base_url",
    [
        "https://foo.lk888.ai",
        "https://api.lk888.ai:443",
        "https://api.lk888.ai/v1",
        "https://api.lk888.ai?target=other",
    ],
)
def test_config_rejects_every_origin_except_fixed_lk888_api(base_url):
    with pytest.raises(ConfigError):
        BackendConfig.from_env(valid_env(LK888_BASE_URL=base_url))


@pytest.mark.parametrize(
    ("overrides", "message"),
    [
        ({"LK888_BASE_URL": "http://api.lk888.ai"}, "must use https"),
        ({"LK888_BASE_URL": "https://"}, "must use https"),
        ({"LK888_BASE_URL": "https://other-provider.example"}, "lk888.ai"),
        ({"PHOTO_AVATAR_BACKEND_HOST": "0.0.0.0"}, "loopback"),
        ({"PHOTO_AVATAR_BACKEND_PORT": "0"}, "port"),
        ({"PHOTO_AVATAR_BACKEND_PORT": "not-a-port"}, "port"),
        ({"LK888_ANALYSIS_MODEL": ""}, "LK888_ANALYSIS_MODEL"),
        ({"LK888_ANALYSIS_MODEL": "other-analysis-model"}, "gpt-4o"),
        ({"LK888_IMAGE_MODEL": ""}, "LK888_IMAGE_MODEL"),
        ({"LK888_IMAGE_MODEL": "other-image-model"}, "gpt-image-2"),
    ],
)
def test_config_rejects_unsafe_or_invalid_values(overrides, message):
    with pytest.raises(ConfigError, match=message):
        BackendConfig.from_env(valid_env(**overrides))


def test_config_normalizes_a_relative_state_directory(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    config = BackendConfig.from_env(
        valid_env(PHOTO_AVATAR_BACKEND_STATE_DIR="relative-output")
    )

    assert config.state_dir == (tmp_path / "relative-output").resolve()


def test_config_rejects_state_directory_inside_tracked_source_tree():
    tracked_source_dir = Path(__file__).resolve().parent

    with pytest.raises(ConfigError, match="state directory"):
        BackendConfig.from_env(
            valid_env(PHOTO_AVATAR_BACKEND_STATE_DIR=str(tracked_source_dir))
        )


def test_example_configuration_contains_only_empty_secret_values():
    example = Path(__file__).resolve().parents[2] / ".env.example"
    entries = dict(
        line.split("=", 1)
        for line in example.read_text(encoding="utf-8").splitlines()
        if line and not line.startswith("#")
    )

    assert entries["LK888_API_KEY"] == ""
    assert entries["PHOTO_AVATAR_BACKEND_TOKEN"] == ""
    assert entries["LK888_ANALYSIS_MODEL"] == "gpt-4o"
    assert entries["LK888_IMAGE_MODEL"] == "gpt-image-2"
    assert entries["PHOTO_AVATAR_BACKEND_HOST"] == "127.0.0.1"
    assert entries["PHOTO_AVATAR_BACKEND_PORT"] == "8787"
    assert entries["PHOTO_AVATAR_BACKEND_STATE_DIR"] == "output/photo-avatar-backend"

"""Configuration for the local, loopback-only photo-avatar backend."""

from dataclasses import dataclass
from pathlib import Path
from typing import Mapping
from urllib.parse import urlparse

from .audit import (
    API_CONTRACT_VERSION,
    PRIVACY_POLICY_VERSION,
    RETENTION_POLICY,
    UPSTREAM_DELETE_API,
)


class ConfigError(ValueError):
    """Raised when backend configuration is incomplete or unsafe."""


_LK888_BASE_URL = "https://api.lk888.ai"


@dataclass(frozen=True)
class BackendConfig:
    lk888_api_key: str
    backend_token: str
    lk888_base_url: str = _LK888_BASE_URL
    analysis_model: str = "gpt-4o"
    image_model: str = "gpt-image-2"
    host: str = "127.0.0.1"
    port: int = 8787
    state_dir: Path = Path("output/photo-avatar-backend")
    provider: str = "lk888"
    model_display_name: str = "GPT-image-2.0"
    api_contract_version: str = API_CONTRACT_VERSION
    privacy_policy_version: str = PRIVACY_POLICY_VERSION
    retention_policy: str = RETENTION_POLICY
    upstream_delete_api: str = UPSTREAM_DELETE_API

    @classmethod
    def from_env(cls, env: Mapping[str, str]) -> "BackendConfig":
        lk888_api_key = _required(env, "LK888_API_KEY")
        backend_token = _required(env, "PHOTO_AVATAR_BACKEND_TOKEN")
        if lk888_api_key == backend_token:
            raise ConfigError("LK888_API_KEY and PHOTO_AVATAR_BACKEND_TOKEN must differ")

        base_url = env.get("LK888_BASE_URL", _LK888_BASE_URL).strip().rstrip("/")
        parsed = urlparse(base_url)
        if parsed.scheme != "https" or not parsed.hostname:
            raise ConfigError("LK888_BASE_URL must use https")
        hostname = parsed.hostname.lower()
        if hostname != "lk888.ai" and not hostname.endswith(".lk888.ai"):
            raise ConfigError("LK888_BASE_URL host must be lk888.ai")
        if parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise ConfigError("LK888_BASE_URL must be an HTTPS origin")
        if base_url != _LK888_BASE_URL:
            raise ConfigError(f"LK888_BASE_URL must be {_LK888_BASE_URL}")
        host = env.get("PHOTO_AVATAR_BACKEND_HOST", "127.0.0.1").strip()
        if host not in {"127.0.0.1", "localhost"}:
            raise ConfigError("development host must be loopback")
        port = _parse_port(env.get("PHOTO_AVATAR_BACKEND_PORT", "8787"))
        analysis_model = _fixed_model(env, "LK888_ANALYSIS_MODEL", "gpt-4o")
        image_model = _fixed_model(env, "LK888_IMAGE_MODEL", "gpt-image-2")
        state_dir = _state_dir(env)

        return cls(
            lk888_api_key=lk888_api_key,
            backend_token=backend_token,
            lk888_base_url=base_url,
            analysis_model=analysis_model,
            image_model=image_model,
            host=host,
            port=port,
            state_dir=state_dir,
        )


def _required(env: Mapping[str, str], name: str) -> str:
    value = env.get(name, "").strip()
    if not value:
        raise ConfigError(f"missing {name}")
    return value


def _fixed_model(env: Mapping[str, str], name: str, expected: str) -> str:
    value = env.get(name, expected).strip()
    if value != expected:
        raise ConfigError(f"{name} must be {expected}")
    return value


def _state_dir(env: Mapping[str, str]) -> Path:
    raw = env.get("PHOTO_AVATAR_BACKEND_STATE_DIR", "output/photo-avatar-backend").strip()
    if not raw:
        raise ConfigError("PHOTO_AVATAR_BACKEND_STATE_DIR must be non-empty")
    path = Path(raw).expanduser()
    resolved = (Path.cwd() / path).resolve() if not path.is_absolute() else path.resolve()

    repository_root = Path(__file__).resolve().parents[4]
    if resolved.is_relative_to(repository_root):
        service_root = Path(__file__).resolve().parents[2]
        allowed_roots = (
            (repository_root / "output").resolve(),
            (service_root / "output").resolve(),
        )
        if not any(resolved.is_relative_to(root) for root in allowed_roots):
            raise ConfigError("state directory inside repository must be under output/")
    return resolved


def _parse_port(value: str) -> int:
    try:
        port = int(value)
    except (TypeError, ValueError) as exc:
        raise ConfigError("port must be an integer between 1 and 65535") from exc
    if not 1 <= port <= 65535:
        raise ConfigError("port must be between 1 and 65535")
    return port

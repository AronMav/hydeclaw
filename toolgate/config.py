"""Configuration management for toolgate providers."""

import json
import logging
import os
import shutil
from pathlib import Path

import httpx
from pydantic import BaseModel, Field

CONFIG_PATH = os.environ.get("CONFIG_PATH", "/app/config/providers.json")
DEFAULT_CONFIG_PATH = os.path.join(os.path.dirname(__file__), "providers.json")

CORE_API_URL = os.environ.get("CORE_API_URL", "http://127.0.0.1:18789")
CORE_AUTH_TOKEN = os.environ.get("HYDECLAW_AUTH_TOKEN", os.environ.get("AUTH_TOKEN", ""))

_log = logging.getLogger("toolgate.config")


class ProviderConfig(BaseModel):
    type: str
    driver: str
    base_url: str = ""
    model: str | None = None
    api_key: str | None = None
    enabled: bool = True
    options: dict = Field(default_factory=dict)


class ProvidersConfig(BaseModel):
    version: int = 1
    active: dict[str, str | None] = Field(default_factory=dict)
    providers: dict[str, ProviderConfig] = Field(default_factory=dict)


def _seed_from_env() -> ProvidersConfig:
    """Create default config from legacy environment variables."""
    whisper_url = os.environ.get("WHISPER_URL", "http://localhost:8300/v1")
    vision_url = os.environ.get("VISION_URL", "https://ollama.com/v1")
    vision_model = os.environ.get("VISION_MODEL", "qwen3.5:397b-cloud")
    ollama_api_key = os.environ.get("OLLAMA_API_KEY", "")

    return ProvidersConfig(
        active={"stt": "local-whisper", "vision": "local-ollama", "tts": None, "imagegen": None},
        providers={
            "local-whisper": ProviderConfig(
                type="stt",
                driver="whisper-local",
                base_url=whisper_url,
                model="Systran/faster-whisper-large-v3",
            ),
            "local-ollama": ProviderConfig(
                type="vision",
                driver="ollama",
                base_url=vision_url,
                model=vision_model,
                api_key=ollama_api_key or None,
            ),
        },
    )


def _load_config_from_api() -> ProvidersConfig | None:
    """Try to load config from Core API (GET /api/media-config).

    Returns the parsed ProvidersConfig on success, or None if unavailable.
    The Core endpoint returns unmasked api_keys and the data in ProvidersConfig format.
    """
    if not CORE_API_URL:
        return None
    try:
        headers: dict[str, str] = {}
        if CORE_AUTH_TOKEN:
            headers["Authorization"] = f"Bearer {CORE_AUTH_TOKEN}"
        resp = httpx.get(
            f"{CORE_API_URL}/api/media-config",
            headers=headers,
            timeout=5.0,
        )
        if resp.status_code == 200:
            data = resp.json()
            config = ProvidersConfig(**data)
            _log.info(
                "Loaded config from Core API: %d providers, active=%s",
                len(config.providers),
                list(config.active.keys()),
            )
            return config
        else:
            _log.warning(
                "Core API /api/media-config returned status %d — falling back to disk",
                resp.status_code,
            )
    except Exception as e:
        _log.warning("Failed to load config from Core API: %s — falling back to disk", e)
    return None


def load_config() -> ProvidersConfig:
    """Load config from Core API first, fallback to disk."""
    # Try Core API (preferred — contains real api_keys from DB)
    config = _load_config_from_api()
    if config is not None:
        return config

    # Fallback: load from disk
    path = Path(CONFIG_PATH)
    if path.exists():
        _log.info("Loading config from disk: %s", path)
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return ProvidersConfig(**data)

    # Try baked-in default
    default = Path(DEFAULT_CONFIG_PATH)
    if default.exists() and str(default) != str(path):
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(default, path)
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return ProvidersConfig(**data)

    # Seed from env vars
    _log.info("No config source available — seeding from environment variables")
    config = _seed_from_env()
    return config

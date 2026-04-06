"""Configuration management for toolgate providers."""

import json
import logging
import os
import shutil
from pathlib import Path

import httpx
from pydantic import BaseModel, Field

CONFIG_PATH = os.environ.get("CONFIG_PATH", "providers.json")
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
    core_url = os.environ.get("CORE_API_URL", CORE_API_URL)
    if not core_url:
        return None
    # Read token at call time (not import time) — process manager sets env after module load
    auth_token = os.environ.get("HYDECLAW_AUTH_TOKEN", os.environ.get("AUTH_TOKEN", ""))
    try:
        headers: dict[str, str] = {}
        if auth_token:
            headers["Authorization"] = f"Bearer {auth_token}"
        resp = httpx.get(
            f"{core_url}/api/media-config",
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
    """Load config: disk file first (written by Core before spawning toolgate),
    then Core API fallback, then env vars."""
    # 1. Disk file (written by Core at startup — most reliable)
    path = Path(CONFIG_PATH)
    if path.exists():
        _log.info("Loading config from disk: %s", path)
        try:
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
            config = ProvidersConfig(**data)
            if config.providers:
                return config
        except Exception as e:
            _log.warning("Failed to parse %s: %s", path, e)

    # 2. Core API fallback
    config = _load_config_from_api()
    if config is not None:
        return config

    # 3. Environment variables
    _log.info("No config source available — seeding from environment variables")
    return _seed_from_env()

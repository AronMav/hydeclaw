"""Shared pytest fixtures for toolgate unit tests."""

import sys
from pathlib import Path

import httpx
import pytest
import pytest_asyncio

# Make toolgate/ importable as top-level (mirrors production import style)
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


@pytest.fixture(autouse=True)
def _clear_legacy_env(monkeypatch):
    """Scrub legacy env vars so they can't affect behavior under test.
    This replaces module reloads; each test runs with a clean env."""
    for var in [
        "WHISPER_URL", "VISION_URL", "VISION_MODEL", "OLLAMA_API_KEY",
        "TTS_BACKEND_URL", "MINIMAX_API_KEY",
        "LLM_API_URL", "LLM_API_KEY", "LLM_MODEL", "LLM_TIMEOUT", "TTS_SKIP_LLM",
    ]:
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setenv("CORE_API_URL", "http://core-test:18789")


@pytest_asyncio.fixture
async def http_client():
    async with httpx.AsyncClient() as client:
        yield client

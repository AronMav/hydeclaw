"""Unit tests for toolgate.config.aload_config."""

import pytest
import respx
import httpx

from config import aload_config, ProvidersConfig


@pytest.mark.asyncio
async def test_aload_config_returns_empty_on_retry_exhaustion(monkeypatch):
    """When Core API returns 5×500, aload_config logs error and returns empty config.
    No env seed — env fallback has been removed."""
    # Shorten retry backoff so the test doesn't take 20 seconds
    import asyncio
    _original_sleep = asyncio.sleep
    monkeypatch.setattr(asyncio, "sleep", lambda _: _original_sleep(0))

    async with respx.mock(assert_all_called=False) as mock:
        mock.get("http://core-test:18789/api/media-config").mock(
            return_value=httpx.Response(500)
        )
        config = await aload_config()
    assert isinstance(config, ProvidersConfig)
    assert config.providers == {}
    assert config.active == {}


@pytest.mark.asyncio
async def test_aload_config_happy_path():
    """Core returns a valid config → parsed correctly."""
    payload = {
        "version": 1,
        "active": {"stt": "local-whisper"},
        "providers": {
            "local-whisper": {
                "type": "stt",
                "driver": "whisper-local",
                "base_url": "http://localhost:8300/v1",
                "model": "faster-whisper-large-v3",
                "enabled": True,
            }
        },
    }
    async with respx.mock(assert_all_called=True) as mock:
        mock.get("http://core-test:18789/api/media-config").mock(
            return_value=httpx.Response(200, json=payload)
        )
        config = await aload_config()
    assert "local-whisper" in config.providers
    assert config.active["stt"] == "local-whisper"

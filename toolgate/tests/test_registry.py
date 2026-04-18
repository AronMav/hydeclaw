"""Unit tests for ProviderRegistry degraded-mode flag."""

import pytest
from registry import ProviderRegistry
from config import ProvidersConfig, ProviderConfig


@pytest.mark.asyncio
async def test_is_degraded_true_when_no_providers_loaded(monkeypatch):
    async def _empty_load():
        return ProvidersConfig()

    monkeypatch.setattr("registry.aload_config", _empty_load)
    reg = ProviderRegistry()
    await reg.aload()
    assert reg.is_degraded() is True


@pytest.mark.asyncio
async def test_is_degraded_false_after_successful_load(monkeypatch):
    async def _populated_load():
        return ProvidersConfig(
            active={"stt": "local-whisper"},
            providers={
                "local-whisper": ProviderConfig(
                    type="stt",
                    driver="whisper-local",
                    base_url="http://localhost:8300/v1",
                    model="faster-whisper-large-v3",
                ),
            },
        )

    monkeypatch.setattr("registry.aload_config", _populated_load)
    reg = ProviderRegistry()
    await reg.aload()
    assert reg.is_degraded() is False

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


from fastapi.testclient import TestClient


@pytest.mark.asyncio
async def test_health_reports_degraded_and_capabilities(monkeypatch):
    """/health must expose degraded flag + per-capability boolean map."""
    async def _empty_load():
        return ProvidersConfig()

    monkeypatch.setattr("registry.aload_config", _empty_load)
    # Re-import app so it picks up the patched registry loader
    import importlib
    import app as app_module
    importlib.reload(app_module)

    with TestClient(app_module.app) as client:
        resp = client.get("/health")
    assert resp.status_code == 200
    body = resp.json()
    assert body["degraded"] is True
    assert body["loaded_providers"] == 0
    assert set(body["capabilities"].keys()) == {"stt", "tts", "vision", "imagegen", "embedding"}
    assert all(v is False for v in body["capabilities"].values())


@pytest.mark.asyncio
async def test_stt_endpoint_returns_structured_503_when_degraded(monkeypatch):
    """When no STT provider is active, /transcribe-url returns structured 503.
    Exercises require_provider() — which all capability endpoints route through."""
    async def _empty_load():
        return ProvidersConfig()

    monkeypatch.setattr("registry.aload_config", _empty_load)
    import importlib
    import app as app_module
    importlib.reload(app_module)

    with TestClient(app_module.app) as client:
        resp = client.post(
            "/transcribe-url",
            json={"audio_url": "http://example/x.mp3"},
        )
    assert resp.status_code == 503
    body = resp.json()
    assert body["error"] == "no_stt_provider"
    assert body["degraded"] is True
    assert "provider" in body["hint"].lower()


@pytest.mark.asyncio
async def test_tts_endpoint_also_uses_structured_503(monkeypatch):
    """Verify the shared dependency produces correct capability-scoped error for TTS too."""
    async def _empty_load():
        return ProvidersConfig()

    monkeypatch.setattr("registry.aload_config", _empty_load)
    import importlib
    import app as app_module
    importlib.reload(app_module)

    with TestClient(app_module.app) as client:
        resp = client.post("/v1/audio/speech", json={"input": "test"})
    assert resp.status_code == 503
    assert resp.json()["error"] == "no_tts_provider"

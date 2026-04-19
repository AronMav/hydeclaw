"""Unit tests for /primitives/bcs/portfolio."""

import pytest
import httpx
import respx
from fastapi.testclient import TestClient


@pytest.fixture
def client():
    from fastapi import FastAPI
    from primitives import bcs

    app = FastAPI()
    app.include_router(bcs.router)
    # Clear module-level cache between tests via the public helper.
    bcs.invalidate_cache()
    return TestClient(app)


def test_bcs_portfolio_rejects_missing_refresh_token(client):
    resp = client.post("/primitives/bcs/portfolio", json={})
    assert resp.status_code == 422


@respx.mock
def test_bcs_portfolio_happy_path(client):
    # Mock refresh + portfolio + (no RT rotation)
    respx.post("https://be.broker.ru/trade-api-keycloak/realms/tradeapi/protocol/openid-connect/token").mock(
        return_value=httpx.Response(200, json={"access_token": "AT1", "refresh_token": "RT1_SAME"})
    )
    respx.get("https://be.broker.ru/trade-api-bff-portfolio/api/v1/portfolio").mock(
        return_value=httpx.Response(200, json={
            "positions": [
                {"instrument": {"ticker": "SBER", "name": "Sberbank"}, "quantity": 10,
                 "marketPrice": 300.0, "profitLoss": 150.0, "dailyPL": 20.0, "dailyPercentPL": 1.5}
            ]
        })
    )

    resp = client.post("/primitives/bcs/portfolio", json={"refresh_token": "RT1_SAME"})
    assert resp.status_code == 200, resp.text
    data = resp.json()
    assert "total_rub" in data
    assert "positions" in data
    assert data["positions"][0]["ticker"] == "SBER"


@respx.mock
def test_bcs_portfolio_rotates_refresh_token_to_vault(client, monkeypatch):
    """When Keycloak returns a new refresh_token, the primitive POSTs it to core /api/secrets."""
    respx.post("https://be.broker.ru/trade-api-keycloak/realms/tradeapi/protocol/openid-connect/token").mock(
        return_value=httpx.Response(200, json={"access_token": "AT1", "refresh_token": "RT2_NEW"})
    )
    respx.get("https://be.broker.ru/trade-api-bff-portfolio/api/v1/portfolio").mock(
        return_value=httpx.Response(200, json={"positions": []})
    )
    # Expect a POST to core /api/secrets to persist the new RT
    secret_route = respx.post("http://core-test:18789/api/secrets").mock(
        return_value=httpx.Response(200, json={"ok": True})
    )

    resp = client.post("/primitives/bcs/portfolio", json={"refresh_token": "RT1_OLD"})
    assert resp.status_code == 200
    assert secret_route.called, "rotated refresh token was not persisted to vault"
    body = secret_route.calls[0].request.read()
    assert b"RT2_NEW" in body


@respx.mock
def test_bcs_portfolio_refreshes_on_401(client):
    """If portfolio returns 401, primitive re-refreshes token and retries once."""
    refresh_route = respx.post("https://be.broker.ru/trade-api-keycloak/realms/tradeapi/protocol/openid-connect/token").mock(
        side_effect=[
            httpx.Response(200, json={"access_token": "AT1", "refresh_token": "RT1"}),
            httpx.Response(200, json={"access_token": "AT2", "refresh_token": "RT1"}),
        ]
    )
    portfolio_route = respx.get("https://be.broker.ru/trade-api-bff-portfolio/api/v1/portfolio").mock(
        side_effect=[
            httpx.Response(401, text="token expired"),
            httpx.Response(200, json={"positions": []}),
        ]
    )
    resp = client.post("/primitives/bcs/portfolio", json={"refresh_token": "RT1"})
    assert resp.status_code == 200
    assert refresh_route.call_count == 2
    assert portfolio_route.call_count == 2

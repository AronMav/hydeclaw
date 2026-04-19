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
    # Mock refresh + portfolio. BCS returns a top-level LIST of positions,
    # each replicated 4x across T0/T1/T2/T365 settlement terms; primitive
    # keeps T0 only. (Verified against live API; tests fabricating a
    # `{"positions": [...]}` dict would mask the real shape — see the
    # production-deploy incident that surfaced this.)
    respx.post("https://be.broker.ru/trade-api-keycloak/realms/tradeapi/protocol/openid-connect/token").mock(
        return_value=httpx.Response(200, json={"access_token": "AT1", "refresh_token": "RT1_SAME"})
    )
    respx.get("https://be.broker.ru/trade-api-bff-portfolio/api/v1/portfolio").mock(
        return_value=httpx.Response(200, json=[
            {
                "term": "T0", "ticker": "SBER", "displayName": "Sberbank",
                "instrumentType": "stock", "quantity": 10, "currentPrice": 300.0,
                "currency": "RUB", "currentValueRub": 3000.0,
                "unrealizedPL": 150.0, "unrealizedPercentPL": 5.0,
                "dailyPL": 20.0, "dailyPercentPL": 0.7,
            },
            # Duplicate at T1 — must be dropped.
            {"term": "T1", "ticker": "SBER", "displayName": "Sberbank", "currentValueRub": 3000.0},
        ])
    )

    resp = client.post("/primitives/bcs/portfolio", json={"refresh_token": "RT1_SAME"})
    assert resp.status_code == 200, resp.text
    data = resp.json()
    assert "total_rub" in data
    assert "positions" in data
    assert len(data["positions"]) == 1, "T1 duplicate should be filtered"
    assert data["positions"][0]["ticker"] == "SBER"
    assert data["positions"][0]["value_rub"] == 3000.0


@respx.mock
def test_bcs_portfolio_rotates_refresh_token_to_vault(client, monkeypatch):
    """When Keycloak returns a new refresh_token, the primitive POSTs it to core /api/secrets."""
    respx.post("https://be.broker.ru/trade-api-keycloak/realms/tradeapi/protocol/openid-connect/token").mock(
        return_value=httpx.Response(200, json={"access_token": "AT1", "refresh_token": "RT2_NEW"})
    )
    respx.get("https://be.broker.ru/trade-api-bff-portfolio/api/v1/portfolio").mock(
        return_value=httpx.Response(200, json=[])
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
            httpx.Response(200, json=[]),
        ]
    )
    resp = client.post("/primitives/bcs/portfolio", json={"refresh_token": "RT1"})
    assert resp.status_code == 200
    assert refresh_route.call_count == 2
    assert portfolio_route.call_count == 2


@respx.mock
def test_bcs_portfolio_invalid_refresh_token_returns_401(client):
    """If Keycloak rejects the refresh_token with 400, primitive returns 401 (not opaque 500)."""
    respx.post("https://be.broker.ru/trade-api-keycloak/realms/tradeapi/protocol/openid-connect/token").mock(
        return_value=httpx.Response(400, json={"error": "invalid_grant"})
    )
    resp = client.post("/primitives/bcs/portfolio", json={"refresh_token": "STALE_RT"})
    assert resp.status_code == 401
    assert "refresh" in resp.json()["detail"].lower()

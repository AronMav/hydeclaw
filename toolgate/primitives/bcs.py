"""BCS Broker portfolio primitive.

Domain-specific OAuth2 refresh flow: Keycloak rotates refresh tokens on each
refresh call, so the primitive persists the new refresh_token back to the
core secrets vault. Access-token is cached module-level; lost on restart.

Exception to the general 'stateless primitive' rule — accepted because the
underlying API mandates it.
"""

import logging

import httpx
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from workspace_helpers import core_api

log = logging.getLogger("toolgate.primitives.bcs")
router = APIRouter(prefix="/primitives/bcs", tags=["primitives"])

AUTH_URL = "https://be.broker.ru/trade-api-keycloak/realms/tradeapi/protocol/openid-connect/token"
PORTFOLIO_URL = "https://be.broker.ru/trade-api-bff-portfolio/api/v1/portfolio"
CLIENT_ID = "trade-api-read"

_access_token: str | None = None


class BcsPortfolioRequest(BaseModel):
    refresh_token: str


def invalidate_cache():
    """Used by app reload hook and tests."""
    global _access_token
    _access_token = None


async def _refresh(http: httpx.AsyncClient, refresh_token: str) -> str:
    """Refresh access token; if Keycloak rotates the refresh token, persist the new one."""
    global _access_token
    resp = await http.post(AUTH_URL, data={
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    })
    resp.raise_for_status()
    data = resp.json()
    _access_token = data["access_token"]

    new_rt = data.get("refresh_token")
    if new_rt and new_rt != refresh_token:
        try:
            await core_api("POST", "/api/secrets", json={
                "name": "BCS_REFRESH_TOKEN",
                "scope": "",
                "value": new_rt,
            })
            log.info("BCS refresh token rotated and persisted to vault")
        except Exception as e:
            log.warning("Failed to persist rotated BCS refresh token: %s", e)

    return new_rt or refresh_token


def _summarize(data: dict) -> dict:
    total_rub = 0.0
    positions = []
    for p in data.get("positions", []):
        instr = p.get("instrument", {}) or {}
        value = p.get("quantity", 0) * p.get("marketPrice", 0.0)
        total_rub += value
        positions.append({
            "ticker": instr.get("ticker"),
            "name": instr.get("name"),
            "quantity": p.get("quantity"),
            "price_rub": p.get("marketPrice"),
            "value_rub": round(value, 2),
            "pnl_rub": round(p.get("profitLoss", 0), 2),
            "daily_pnl_rub": round(p.get("dailyPL", 0), 2),
            "daily_pnl_pct": round(p.get("dailyPercentPL", 0), 2),
        })
    return {"total_rub": round(total_rub, 2), "positions": positions}


@router.post("/portfolio")
async def portfolio(req: BcsPortfolioRequest):
    """Fetch the BCS portfolio. Handles OAuth2 refresh and one-time retry on 401."""
    global _access_token

    async with httpx.AsyncClient(timeout=20) as http:
        current_rt = req.refresh_token

        if not _access_token:
            current_rt = await _refresh(http, current_rt)

        headers = {"Authorization": f"Bearer {_access_token}"}
        resp = await http.get(PORTFOLIO_URL, headers=headers)

        if resp.status_code == 401:
            current_rt = await _refresh(http, current_rt)
            headers = {"Authorization": f"Bearer {_access_token}"}
            resp = await http.get(PORTFOLIO_URL, headers=headers)

        if resp.status_code != 200:
            raise HTTPException(resp.status_code, resp.text)

        return _summarize(resp.json())

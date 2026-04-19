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
    try:
        resp.raise_for_status()
    except httpx.HTTPStatusError as e:
        # Surface stale/expired refresh tokens as 401 so the agent can prompt the
        # user to re-authenticate instead of seeing an opaque 500.
        status = 401 if 400 <= e.response.status_code < 500 else 502
        raise HTTPException(status, f"BCS refresh failed: {e.response.text}") from e
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


def _summarize(raw: list) -> dict:
    """Compact portfolio: deduplicate by ticker (keep T0), drop noise fields.

    BCS returns each position 4x for T0/T1/T2/T365 settlement terms — keep T0 only.
    The top-level BCS response is a list, not a dict.
    """
    if not isinstance(raw, list):
        return {"total_rub": 0.0, "positions": []}

    t0 = [p for p in raw if isinstance(p, dict) and p.get("term") == "T0"]

    positions = []
    total_rub = 0.0

    for p in sorted(t0, key=lambda x: x.get("currentValueRub", 0), reverse=True):
        value_rub = p.get("currentValueRub", 0)
        total_rub += value_rub
        positions.append({
            "ticker": p.get("ticker"),
            "name": p.get("displayName"),
            "type": p.get("instrumentType") or p.get("upperType"),
            "qty": p.get("quantity"),
            "price": p.get("currentPrice"),
            "currency": p.get("currency"),
            "value_rub": round(value_rub, 2),
            "pnl_rub": round(p.get("unrealizedPL", 0), 2),
            "pnl_pct": round(p.get("unrealizedPercentPL", 0), 2),
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

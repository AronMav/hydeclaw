"""BCS Portfolio workspace router — gets portfolio from BCS via Trade API."""
import logging
import httpx
from fastapi import APIRouter, HTTPException
from workspace_helpers import get_secret, core_api

log = logging.getLogger(__name__)

router = APIRouter()

AUTH_URL = "https://be.broker.ru/trade-api-keycloak/realms/tradeapi/protocol/openid-connect/token"
PORTFOLIO_URL = "https://be.broker.ru/trade-api-bff-portfolio/api/v1/portfolio"
CLIENT_ID = "trade-api-read"

_access_token = None


def invalidate_cache():
    """Called by app reload to clear cached tokens."""
    global _access_token
    _access_token = None


async def _refresh_token(http, refresh_token):
    global _access_token
    resp = await http.post(AUTH_URL, data={
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    })
    resp.raise_for_status()
    data = resp.json()
    _access_token = data["access_token"]

    # Keycloak rotates refresh tokens — persist the new one back to vault
    new_rt = data.get("refresh_token")
    if new_rt and new_rt != refresh_token:
        try:
            await core_api("POST", "/api/secrets", json={
                "name": "BCS_REFRESH_TOKEN",
                "value": new_rt,
            })
        except Exception as e:
            log.warning("failed to persist rotated BCS refresh token: %s", e)

    return _access_token


def _summarize(raw: list) -> dict:
    """Compact portfolio: deduplicate by ticker (keep T0), drop noise fields."""
    # BCS returns each position 4x for T0/T1/T2/T365 settlement terms — keep T0 only
    t0 = [p for p in raw if p.get("term") == "T0"]

    positions = []
    total_rub = 0.0

    for p in sorted(t0, key=lambda x: x.get("currentValueRub", 0), reverse=True):
        value_rub = p.get("currentValueRub", 0)
        total_rub += value_rub
        entry = {
            "ticker": p["ticker"],
            "name": p["displayName"],
            "type": p.get("instrumentType") or p.get("upperType"),
            "qty": p["quantity"],
            "price": p["currentPrice"],
            "currency": p["currency"],
            "value_rub": round(value_rub, 2),
            "pnl_rub": round(p.get("unrealizedPL", 0), 2),
            "pnl_pct": round(p.get("unrealizedPercentPL", 0), 2),
            "daily_pnl_rub": round(p.get("dailyPL", 0), 2),
            "daily_pnl_pct": round(p.get("dailyPercentPL", 0), 2),
        }
        positions.append(entry)

    return {
        "total_rub": round(total_rub, 2),
        "positions": positions,
    }


@router.get("/bcs/portfolio")
async def bcs_portfolio():
    refresh_token = await get_secret("BCS_REFRESH_TOKEN")
    if not refresh_token:
        raise HTTPException(500, "BCS_REFRESH_TOKEN not found in secrets")

    global _access_token
    async with httpx.AsyncClient(timeout=20) as http:
        if not _access_token:
            await _refresh_token(http, refresh_token)

        headers = {"Authorization": f"Bearer {_access_token}"}
        resp = await http.get(PORTFOLIO_URL, headers=headers)

        if resp.status_code == 401:
            await _refresh_token(http, refresh_token)
            headers = {"Authorization": f"Bearer {_access_token}"}
            resp = await http.get(PORTFOLIO_URL, headers=headers)

        if resp.status_code != 200:
            raise HTTPException(resp.status_code, resp.text)

        return _summarize(resp.json())

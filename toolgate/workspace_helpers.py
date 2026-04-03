"""Helpers for workspace routers — simplifies common operations."""
import os
import httpx

AUTH_TOKEN = os.environ.get("AUTH_TOKEN", "")
CORE_URL = os.environ.get("CORE_URL", "http://127.0.0.1:18789")

# Shared client — reused across all requests (avoids TCP+TLS per call)
_client = httpx.AsyncClient(timeout=15)

def _headers() -> dict:
    """Auth headers for core API (needed when core runs on host, not in Docker)."""
    if AUTH_TOKEN:
        return {"Authorization": f"Bearer {AUTH_TOKEN}"}
    return {}


async def get_secret(name: str, scope: str = "") -> str:
    """Read a secret from HydeClaw vault by name.

    Usage in a workspace router:
        from workspace_helpers import get_secret
        token = await get_secret("MY_API_KEY")
    """
    params = f"?reveal=true&scope={scope}" if scope else "?reveal=true"
    resp = await _client.get(f"{CORE_URL}/api/secrets/{name}{params}", headers=_headers())
    if resp.status_code == 200:
        return resp.json().get("value", "")
    return ""


async def core_api(method: str, path: str, json: dict | None = None) -> dict:
    """Call HydeClaw core API.

    Usage: data = await core_api("GET", "/api/agents")
    """
    resp = await _client.request(method, f"{CORE_URL}{path}", headers=_headers(), json=json)
    resp.raise_for_status()
    return resp.json()

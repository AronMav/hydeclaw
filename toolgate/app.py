"""Toolgate — Universal tool processing hub.

Supports multiple providers for STT, Vision, TTS, and Image Generation.
Utility services: document text extraction, URL content fetching.
Configuration loaded from Core API at startup.
"""

import logging
import os
import sys
import secrets
import time
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
import httpx

log = logging.getLogger("toolgate")

from registry import ProviderRegistry, CAPABILITIES

registry = ProviderRegistry()
http_client: httpx.AsyncClient = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    global http_client
    http_client = httpx.AsyncClient(timeout=120.0)
    app.state.registry = registry
    app.state.http_client = http_client
    await registry.aload()
    yield
    if http_client:
        await http_client.aclose()


app = FastAPI(lifespan=lifespan)

AUTH_TOKEN = os.environ.get("AUTH_TOKEN", "")
INTERNAL_NETWORK = os.environ.get("INTERNAL_NETWORK", "127.0.0.0/8")
# Paths/prefixes that don't require authentication
PUBLIC_PATHS = {"/health"}

import ipaddress
_internal_nets = [ipaddress.ip_network(n.strip()) for n in INTERNAL_NETWORK.split(",")]


def _is_internal(client_host: str) -> bool:
    """Check if request comes from internal/trusted network."""
    try:
        addr = ipaddress.ip_address(client_host)
        return any(addr in net for net in _internal_nets)
    except ValueError:
        return False


@app.middleware("http")
async def auth_middleware(request: Request, call_next):
    path = request.url.path
    if AUTH_TOKEN and path not in PUBLIC_PATHS:
        # Skip auth for inter-container traffic on Docker network
        if _is_internal(request.client.host if request.client else ""):
            return await call_next(request)
        auth = request.headers.get("authorization", "")
        expected = f"Bearer {AUTH_TOKEN}"
        if not auth or not secrets.compare_digest(auth, expected):
            return JSONResponse(status_code=401, content={"error": "unauthorized"})
    return await call_next(request)


@app.middleware("http")
async def log_requests(request: Request, call_next):
    start = time.monotonic()
    response = await call_next(request)
    elapsed_ms = (time.monotonic() - start) * 1000
    log.info("%s %s → %d (%.0fms)", request.method, request.url.path, response.status_code, elapsed_ms)
    return response


# Mount routers
from routers import stt, vision, tts, imagegen, embedding, documents, fetch, bcs_portfolio, email, calendar
app.include_router(stt.router)
app.include_router(vision.router)
app.include_router(tts.router)
app.include_router(imagegen.router)
app.include_router(embedding.router)
app.include_router(documents.router)
app.include_router(fetch.router)
app.include_router(bcs_portfolio.router)
app.include_router(email.router)
app.include_router(calendar.router)


@app.get("/health")
async def health():
    active = {}
    for cap in CAPABILITIES:
        p = registry.get_active(cap)
        active[cap] = p.name if p else None
    return {"status": "ok", "active_providers": active}


@app.post("/reload")
async def reload_providers():
    """Reload provider configuration and invalidate router caches."""
    await registry.areload()
    # Invalidate cached credentials in workspace routers
    _invalidate_router_caches()
    active = {}
    for cap in CAPABILITIES:
        p = registry.get_active(cap)
        active[cap] = p.name if p else None
    log.info("Provider config reloaded via /reload endpoint")
    return {"ok": True, "active_providers": active}


def _invalidate_router_caches():
    """Clear cached tokens/state in workspace routers that read secrets."""
    try:
        from routers.bcs_portfolio import invalidate_cache
        invalidate_cache()
    except (ImportError, AttributeError):
        pass

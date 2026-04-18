"""Shared FastAPI dependencies for toolgate routers."""

from fastapi import Request
from fastapi.responses import JSONResponse


class _DegradedResponse(Exception):
    """Signals that the shared dependency wants to short-circuit with a 503.
    Caught by the FastAPI exception handler registered in app.py."""

    def __init__(self, capability: str, degraded: bool):
        self.capability = capability
        self.degraded = degraded


def require_provider(capability: str):
    """FastAPI dependency returning the active provider, or raising a structured 503.
    The body contains `{error, degraded, hint}` so callers can distinguish
    'no provider configured' vs 'core unreachable' states."""
    def _dep(request: Request):
        registry = request.app.state.registry
        provider = registry.get_active(capability)
        if not provider:
            raise _DegradedResponse(capability, registry.is_degraded())
        return provider
    return _dep


def degraded_response(exc: _DegradedResponse) -> JSONResponse:
    """Build the 503 JSON response for a `_DegradedResponse`."""
    hint = (
        f"Core API is unreachable — {exc.capability} endpoints will resume once Core recovers"
        if exc.degraded
        else f"no {exc.capability} provider is active — configure one in Core UI"
    )
    return JSONResponse(
        status_code=503,
        content={
            "error": f"no_{exc.capability}_provider",
            "degraded": exc.degraded,
            "hint": hint,
        },
    )

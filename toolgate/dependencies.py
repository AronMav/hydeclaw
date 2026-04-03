"""Shared FastAPI dependencies for toolgate routers."""

from fastapi import HTTPException, Request


def require_provider(capability: str):
    """FastAPI dependency that returns the active provider or raises 503."""
    def _dep(request: Request):
        provider = request.app.state.registry.get_active(capability)
        if not provider:
            raise HTTPException(503, f"No active {capability} provider configured")
        return provider
    return _dep

"""Embedding endpoint - proxies to active embedding provider."""
from fastapi import APIRouter, Request
from fastapi.responses import JSONResponse
import logging

log = logging.getLogger(__name__)
router = APIRouter()


@router.post("/v1/embeddings")
async def embeddings(request: Request):
    registry = request.app.state.registry
    provider = registry.get_active("embedding")
    if provider is None:
        return JSONResponse(
            status_code=503,
            content={"error": "no active embedding provider"},
        )

    body = await request.json()
    texts = body.get("input", [])
    model = body.get("model")

    if isinstance(texts, str):
        texts = [texts]

    if not texts:
        return JSONResponse(
            status_code=400,
            content={"error": "input is required"},
        )

    try:
        http = request.app.state.http_client
        vectors = await provider.embed(http, texts, model)
        data = [
            {"object": "embedding", "index": i, "embedding": vec}
            for i, vec in enumerate(vectors)
        ]
        return {"object": "list", "data": data, "model": model or ""}
    except Exception as e:
        log.exception("embedding failed")
        return JSONResponse(
            status_code=502,
            content={"error": str(e)},
        )

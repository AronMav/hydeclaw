"""Embedding endpoint - proxies to active embedding provider."""
from fastapi import APIRouter, Depends, Request
from fastapi.responses import JSONResponse
import logging

from dependencies import require_provider

log = logging.getLogger(__name__)
router = APIRouter()


@router.post("/v1/embeddings")
async def embeddings(
    request: Request,
    provider=Depends(require_provider("embedding")),
):
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
        actual_model = model or getattr(provider, "model", "") or ""
        return {"object": "list", "data": data, "model": actual_model}
    except Exception as e:
        log.exception("embedding failed")
        return JSONResponse(
            status_code=502,
            content={"error": str(e)},
        )

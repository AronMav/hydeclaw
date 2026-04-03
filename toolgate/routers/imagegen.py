"""Image Generation endpoint."""

from fastapi import APIRouter, Request, Depends
from fastapi.responses import JSONResponse, Response
from pydantic import BaseModel
from typing import Optional

import httpx

from dependencies import require_provider

router = APIRouter(tags=["imagegen"])


class ImageGenRequest(BaseModel):
    prompt: str
    size: Optional[str] = "1024x1024"
    model: Optional[str] = None
    quality: Optional[str] = "standard"


@router.post("/generate-image")
async def generate_image(
    body: ImageGenRequest,
    request: Request,
    provider=Depends(require_provider("imagegen")),
):
    try:
        image_bytes = await provider.generate(
            request.app.state.http_client, body.prompt,
            body.size or "1024x1024", body.model,
            body.quality or "standard",
        )
        return Response(content=image_bytes, media_type="image/png")
    except httpx.HTTPStatusError as e:
        return JSONResponse(status_code=e.response.status_code,
                            content={"error": f"ImageGen error: {e.response.text}"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"ImageGen error: {e}"})

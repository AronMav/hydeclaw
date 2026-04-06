"""Vision (Image Description) endpoints."""

import logging

from fastapi import APIRouter, UploadFile, File, Form, Request, Depends
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from typing import Optional

import httpx

from dependencies import require_provider
from helpers import default_vision_prompt, resolve_content_type, download_limited, check_upload_size, log_provider

log = logging.getLogger("toolgate.vision")

VISION_MAX_BYTES = 20 * 1024 * 1024  # 20 MB

router = APIRouter(tags=["vision"])


@router.post("/describe")
async def describe(
    request: Request,
    file: UploadFile = File(...),
    prompt: str = Form(default=""),
    language: str = Form(default="ru"),
    provider=Depends(require_provider("vision")),
):
    log_provider(log, provider)
    image_bytes = await file.read()

    size_err = check_upload_size(image_bytes, VISION_MAX_BYTES, "Image")
    if size_err:
        return size_err

    content_type = resolve_content_type(image_bytes, file.content_type or "")
    vision_prompt = prompt.strip() if prompt.strip() else default_vision_prompt(language)

    try:
        text = await provider.describe(
            request.app.state.http_client, image_bytes, content_type, vision_prompt,
        )
        return {
            "description": text,
            "provider": provider.name,
            "model": getattr(provider, "model", ""),
        }
    except httpx.HTTPStatusError as e:
        return JSONResponse(status_code=e.response.status_code,
                            content={"error": f"Vision error: {e.response.text}"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"Vision error: {e}"})


class DescribeUrlRequest(BaseModel):
    image_url: str
    question: Optional[str] = None
    language: Optional[str] = "ru"


@router.post("/describe-url")
async def describe_url(
    body: DescribeUrlRequest,
    request: Request,
    provider=Depends(require_provider("vision")),
):
    log_provider(log, provider)
    http = request.app.state.http_client
    try:
        image_bytes, raw_ct = await download_limited(http, body.image_url, max_bytes=VISION_MAX_BYTES)
    except httpx.TimeoutException:
        return JSONResponse(status_code=504, content={"error": "Image URL timed out (10s)"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"Failed to download image: {e}"})

    if len(image_bytes) < 100:
        return JSONResponse(status_code=400,
                            content={"error": f"Downloaded content too small ({len(image_bytes)} bytes)"})

    content_type = resolve_content_type(image_bytes, raw_ct)
    q = (body.question or "").strip()
    vision_prompt = q if q else default_vision_prompt(body.language or "ru")

    try:
        text = await provider.describe(
            http, image_bytes, content_type, vision_prompt,
        )
        return {
            "description": text,
            "provider": provider.name,
            "model": getattr(provider, "model", ""),
        }
    except httpx.HTTPStatusError as e:
        return JSONResponse(status_code=e.response.status_code,
                            content={"error": f"Vision error: {e.response.text}"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"Vision error: {e}"})

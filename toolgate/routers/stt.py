"""STT (Speech-to-Text) endpoints."""

import logging

from fastapi import APIRouter, UploadFile, File, Form, Request, Depends
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from typing import Optional

import httpx

from dependencies import require_provider

log = logging.getLogger("toolgate.stt")

STT_MAX_BYTES = 25 * 1024 * 1024  # 25 MB (Whisper API limit)

router = APIRouter(tags=["stt"])


@router.post("/transcribe")
async def transcribe(
    request: Request,
    file: UploadFile = File(...),
    model: str = Form(default=None),
    language: str = Form(default="ru"),
    provider=Depends(require_provider("stt")),
):
    log.info("Using provider: %s model=%s", provider.name, getattr(provider, "model", ""))
    audio_bytes = await file.read()

    if len(audio_bytes) > STT_MAX_BYTES:
        return JSONResponse(
            status_code=413,
            content={"error": f"Audio file too large ({len(audio_bytes)} bytes). Max 25 MB."},
        )

    try:
        text = await provider.transcribe(
            request.app.state.http_client, audio_bytes,
            file.filename or "audio.ogg", language, model,
        )
        return {"text": text}
    except httpx.HTTPStatusError as e:
        return JSONResponse(status_code=e.response.status_code,
                            content={"error": f"STT error: {e.response.text}"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"STT error: {e}"})


class TranscribeUrlRequest(BaseModel):
    audio_url: str
    language: Optional[str] = "ru"
    model: Optional[str] = None


@router.post("/transcribe-url")
async def transcribe_url(
    body: TranscribeUrlRequest,
    request: Request,
    provider=Depends(require_provider("stt")),
):
    log.info("Using provider: %s model=%s", provider.name, getattr(provider, "model", ""))
    http = request.app.state.http_client
    from helpers import validate_url_ssrf
    validate_url_ssrf(body.audio_url)
    try:
        resp = await http.get(body.audio_url, follow_redirects=True)
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"Failed to download audio: {e}"})

    if resp.status_code != 200:
        return JSONResponse(status_code=502,
                            content={"error": f"Failed to download audio: HTTP {resp.status_code}"})

    audio_bytes = resp.content

    if len(audio_bytes) > STT_MAX_BYTES:
        return JSONResponse(
            status_code=413,
            content={"error": f"Audio file too large ({len(audio_bytes)} bytes). Max 25 MB."},
        )

    filename = body.audio_url.split("/")[-1].split("?")[0] or "audio.ogg"

    try:
        text = await provider.transcribe(
            http, audio_bytes, filename, body.language or "ru", body.model,
        )
        return {"text": text}
    except httpx.HTTPStatusError as e:
        return JSONResponse(status_code=e.response.status_code,
                            content={"error": f"STT error: {e.response.text}"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"STT error: {e}"})

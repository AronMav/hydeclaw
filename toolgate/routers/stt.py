"""STT (Speech-to-Text) endpoints."""

import logging

from fastapi import APIRouter, UploadFile, File, Form, Request, Depends
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from typing import Optional

import httpx

from dependencies import require_provider
from helpers import download_limited, check_upload_size, log_provider

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
    log_provider(log, provider)
    audio_bytes = await file.read()

    size_err = check_upload_size(audio_bytes, STT_MAX_BYTES, "Audio file")
    if size_err:
        return size_err

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
    log_provider(log, provider)
    http = request.app.state.http_client
    try:
        audio_bytes, _ = await download_limited(http, body.audio_url, max_bytes=STT_MAX_BYTES)
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"Failed to download audio: {e}"})

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

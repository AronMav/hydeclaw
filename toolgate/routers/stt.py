"""STT (Speech-to-Text) endpoints."""

from fastapi import APIRouter, UploadFile, File, Form, Request, Depends
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from typing import Optional

import httpx

from dependencies import require_provider

router = APIRouter(tags=["stt"])


@router.post("/transcribe")
async def transcribe(
    request: Request,
    file: UploadFile = File(...),
    model: str = Form(default=None),
    language: str = Form(default="ru"),
    provider=Depends(require_provider("stt")),
):
    audio_bytes = await file.read()
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


@router.post("/transcribe-url")
async def transcribe_url(
    body: TranscribeUrlRequest,
    request: Request,
    provider=Depends(require_provider("stt")),
):
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
    filename = body.audio_url.split("/")[-1].split("?")[0] or "audio.ogg"

    try:
        text = await provider.transcribe(
            http, audio_bytes, filename, body.language or "ru",
        )
        return {"text": text}
    except httpx.HTTPStatusError as e:
        return JSONResponse(status_code=e.response.status_code,
                            content={"error": f"STT error: {e.response.text}"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"STT error: {e}"})

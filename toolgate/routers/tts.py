"""TTS (Text-to-Speech) endpoints."""

import logging

from fastapi import APIRouter, Request, Depends
from fastapi.responses import JSONResponse, Response
from pydantic import BaseModel
from typing import Optional

import httpx

from dependencies import require_provider
from normalize import normalize_text

log = logging.getLogger("toolgate.tts")

router = APIRouter(tags=["tts"])


@router.get("/audio/voices")
async def list_voices(
    request: Request,
    provider=Depends(require_provider("tts")),
):
    """Proxy voice listing from upstream TTS server."""
    http = request.app.state.http_client
    try:
        resp = await http.get(f"{provider.base_url}/v1/audio/voices", timeout=5.0)
        resp.raise_for_status()
        return resp.json()
    except Exception as e:
        log.warning("Failed to fetch voices: %s", e)
        return {"voices": []}


class TTSRequest(BaseModel):
    text: str
    voice: Optional[str] = None
    model: Optional[str] = None
    response_format: Optional[str] = "mp3"


@router.post("/tts")
async def tts(
    body: TTSRequest,
    request: Request,
    provider=Depends(require_provider("tts")),
):
    try:
        audio_bytes = await provider.synthesize(
            request.app.state.http_client, body.text,
            body.voice or "", body.model,
            body.response_format or "mp3",
        )
        media_type = "audio/mpeg" if (body.response_format or "mp3") == "mp3" else "audio/ogg"
        return Response(content=audio_bytes, media_type=media_type)
    except httpx.HTTPStatusError as e:
        return JSONResponse(status_code=e.response.status_code,
                            content={"error": f"TTS error: {e.response.text}"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"TTS error: {e}"})


class OpenAISpeechRequest(BaseModel):
    model: Optional[str] = None
    input: str
    voice: Optional[str] = None
    response_format: Optional[str] = "mp3"


@router.post("/v1/audio/speech")
async def openai_speech(
    body: OpenAISpeechRequest,
    request: Request,
    provider=Depends(require_provider("tts")),
):
    """OpenAI-compatible TTS with full Russian text normalization."""
    http = request.app.state.http_client
    text = body.input
    if text:
        normalized = await normalize_text(http, text)
        if normalized != text:
            log.info("Normalized TTS (%d→%d chars): %s", len(text), len(normalized), normalized[:200])
        text = normalized

    try:
        audio_bytes = await provider.synthesize(
            http, text,
            body.voice or "", body.model,
            body.response_format or "mp3",
        )
        media_type = "audio/mpeg" if (body.response_format or "mp3") == "mp3" else "audio/ogg"
        return Response(content=audio_bytes, media_type=media_type)
    except httpx.HTTPStatusError as e:
        return JSONResponse(status_code=e.response.status_code,
                            content={"error": f"TTS error: {e.response.text}"})
    except Exception as e:
        return JSONResponse(status_code=502, content={"error": f"TTS error: {e}"})

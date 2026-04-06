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
    """List available voices from the TTS provider."""
    http = request.app.state.http_client

    # Prefer a native list_voices() method on the provider instance
    if hasattr(provider, "list_voices"):
        try:
            return await provider.list_voices(http)
        except Exception as e:
            log.warning("list_voices() failed on %s: %s", provider.name, e)
            return {"voices": [], "note": "Voice listing unavailable for this provider"}

    # Fallback: try proxying to base_url if provider exposes one (e.g. local Qwen TTS server)
    base_url = getattr(provider, "base_url", None)
    if base_url:
        try:
            resp = await http.get(f"{base_url}/v1/audio/voices", timeout=5.0)
            resp.raise_for_status()
            return resp.json()
        except Exception as e:
            log.warning("Failed to fetch voices from %s: %s", base_url, e)

    return {"voices": [], "note": "This provider does not support voice listing"}


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
    log.info("Using provider: %s model=%s", provider.name, getattr(provider, "model", ""))
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
    log.info("Using provider: %s model=%s", provider.name, getattr(provider, "model", ""))
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

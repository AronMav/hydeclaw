"""OpenAI Whisper STT provider."""

import httpx


class OpenAISTT:
    name = "OpenAI Whisper"

    def __init__(self, base_url: str = "", api_key: str | None = None,
                 model: str | None = None, options: dict | None = None):
        self.base_url = (base_url or "https://api.openai.com/v1").rstrip("/")
        self.api_key = api_key or ""
        self.model = model or "whisper-1"

    async def transcribe(self, http: httpx.AsyncClient, audio_bytes: bytes,
                         filename: str, language: str,
                         model: str | None = None) -> str:
        resp = await http.post(
            f"{self.base_url}/audio/transcriptions",
            headers={"Authorization": f"Bearer {self.api_key}"},
            files={"file": (filename, audio_bytes, "audio/ogg")},
            data={"model": model or self.model, "language": language},
        )
        resp.raise_for_status()
        return resp.json().get("text", "")

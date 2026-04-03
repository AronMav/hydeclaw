"""Microsoft Edge TTS provider (free, no API key required)."""

import asyncio
import io

import httpx


class EdgeTTS:
    name = "Edge TTS"

    def __init__(self, base_url: str = "", api_key: str | None = None,
                 model: str | None = None, options: dict | None = None):
        opts = options or {}
        self.default_voice = opts.get("voice", "ru-RU-SvetlanaNeural")
        self.rate = opts.get("rate", "+0%")
        self.pitch = opts.get("pitch", "+0Hz")
        self.volume = opts.get("volume", "+0%")

    async def synthesize(self, http: httpx.AsyncClient, text: str,
                         voice: str, model: str | None = None,
                         response_format: str = "mp3") -> bytes:
        import edge_tts

        voice_name = voice or self.default_voice
        communicate = edge_tts.Communicate(
            text, voice_name,
            rate=self.rate, pitch=self.pitch, volume=self.volume,
        )

        buf = io.BytesIO()
        async for chunk in communicate.stream():
            if chunk["type"] == "audio":
                buf.write(chunk["data"])

        return buf.getvalue()

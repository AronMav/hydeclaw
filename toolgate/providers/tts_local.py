"""Local Qwen3-TTS provider."""

import httpx


class Qwen3TTS:
    name = "Qwen3-TTS"

    def __init__(self, base_url: str = "", api_key: str | None = None,
                 model: str | None = None, options: dict | None = None):
        self.base_url = (base_url or "http://localhost:8880").rstrip("/")
        self.model = model or "tts-1-ru"
        opts = options or {}
        self.default_voice = opts.get("voice", "nova")
        self.normalize = opts.get("normalize", False)
        self.llm_api_url = opts.get("llm_api_url", "")
        self.llm_api_key = opts.get("llm_api_key", "")
        self.llm_model = opts.get("llm_model", "MiniMax-M2.5")

    async def _normalize_text(self, http: httpx.AsyncClient, text: str) -> str:
        """Normalize Russian text for TTS via LLM (expand abbreviations, numbers, etc.)."""
        if not self.normalize or not self.llm_api_url:
            return text

        from normalize import normalize_for_tts
        return await normalize_for_tts(
            http, text,
            api_url=self.llm_api_url,
            api_key=self.llm_api_key,
            model=self.llm_model,
        )

    async def synthesize(self, http: httpx.AsyncClient, text: str,
                         voice: str, model: str | None = None,
                         response_format: str = "mp3") -> bytes:
        processed = await self._normalize_text(http, text)
        resolved_voice = voice if voice else self.default_voice

        resp = await http.post(
            f"{self.base_url}/v1/audio/speech",
            json={
                "model": model or self.model,
                "input": processed,
                "voice": resolved_voice,
                "response_format": response_format,
            },
        )
        resp.raise_for_status()
        return resp.content

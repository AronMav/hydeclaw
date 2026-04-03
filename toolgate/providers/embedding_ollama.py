"""Ollama-compatible embedding provider (OpenAI /v1/embeddings API)."""
import httpx
import logging

log = logging.getLogger(__name__)


class OllamaEmbedding:
    name = "Ollama Embedding"

    def __init__(
        self,
        base_url: str,
        api_key: str | None = None,
        model: str | None = None,
        options: dict | None = None,
    ):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key or ""
        self.model = model or "qwen3-embedding:4b"

    async def embed(
        self,
        http: httpx.AsyncClient,
        texts: list[str],
        model: str | None = None,
    ) -> list[list[float]]:
        url = f"{self.base_url}/embeddings"
        headers = {}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        body = {"model": model or self.model, "input": texts}
        resp = await http.post(url, json=body, headers=headers, timeout=60.0)
        resp.raise_for_status()
        data = resp.json()

        return [d["embedding"] for d in data["data"]]

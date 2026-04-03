"""Base protocols for toolgate providers."""

from typing import Protocol, runtime_checkable

import httpx


@runtime_checkable
class STTProvider(Protocol):
    name: str

    async def transcribe(
        self,
        http: httpx.AsyncClient,
        audio_bytes: bytes,
        filename: str,
        language: str,
        model: str | None = None,
    ) -> str: ...


@runtime_checkable
class VisionProvider(Protocol):
    name: str

    async def describe(
        self,
        http: httpx.AsyncClient,
        image_bytes: bytes,
        content_type: str,
        prompt: str,
        max_tokens: int = 2000,
    ) -> str: ...


@runtime_checkable
class TTSProvider(Protocol):
    name: str

    async def synthesize(
        self,
        http: httpx.AsyncClient,
        text: str,
        voice: str,
        model: str | None = None,
        response_format: str = "mp3",
    ) -> bytes: ...


@runtime_checkable
class ImageGenProvider(Protocol):
    name: str

    async def generate(
        self,
        http: httpx.AsyncClient,
        prompt: str,
        size: str = "1024x1024",
        model: str | None = None,
        quality: str = "standard",
    ) -> bytes: ...


@runtime_checkable
class EmbeddingProvider(Protocol):
    name: str

    async def embed(
        self,
        http: httpx.AsyncClient,
        texts: list[str],
        model: str | None = None,
    ) -> list[list[float]]: ...

"""Summarize — MCP server for URL/text summarization."""

import os
import httpx
from readability import Document
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse

LLM_URL = os.environ.get("LLM_URL", "https://api.minimax.io/v1/chat/completions")
LLM_API_KEY = os.environ.get("MINIMAX_API_KEY", "")
LLM_MODEL = os.environ.get("LLM_MODEL", "MiniMax-M2.5")

app = FastAPI()
http_client: httpx.AsyncClient = None


@app.on_event("startup")
async def startup():
    global http_client
    http_client = httpx.AsyncClient(timeout=60.0, follow_redirects=True)


@app.on_event("shutdown")
async def shutdown():
    if http_client:
        await http_client.aclose()


MCP_TOOLS = [
    {
        "name": "summarize_url",
        "description": "Fetch a URL and return a concise summary of its content in Russian.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to summarize"},
                "max_length": {"type": "integer", "default": 500, "description": "Max summary length in words"},
            },
            "required": ["url"],
        },
    },
    {
        "name": "summarize_text",
        "description": "Summarize provided text in Russian.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Text to summarize"},
                "max_length": {"type": "integer", "default": 500, "description": "Max summary length in words"},
            },
            "required": ["text"],
        },
    },
]


async def fetch_readable(url: str) -> str:
    """Fetch URL and extract readable text."""
    resp = await http_client.get(url, headers={
        "User-Agent": "Mozilla/5.0 (compatible; HydeClaw/1.0)",
    })
    resp.raise_for_status()
    doc = Document(resp.text)
    text = doc.summary()
    # Strip HTML tags
    import re
    text = re.sub(r"<[^>]+>", " ", text)
    text = re.sub(r"\s+", " ", text).strip()
    # Truncate to ~8000 chars for LLM context
    if len(text) > 8000:
        text = text[:8000] + "..."
    return text


async def summarize_via_llm(text: str, max_length: int = 500) -> str:
    """Summarize text via LLM."""
    if not LLM_API_KEY:
        # Fallback: return truncated text
        return text[:max_length * 5] + "..." if len(text) > max_length * 5 else text

    resp = await http_client.post(LLM_URL, json={
        "model": LLM_MODEL,
        "messages": [
            {"role": "system", "content": f"Сделай краткое резюме текста на русском языке. Максимум {max_length} слов. Выдели ключевые факты."},
            {"role": "user", "content": text},
        ],
        "temperature": 0.3,
        "max_tokens": max_length * 4,
    }, headers={
        "Authorization": f"Bearer {LLM_API_KEY}",
        "Content-Type": "application/json",
    })
    resp.raise_for_status()
    data = resp.json()
    return data["choices"][0]["message"]["content"].strip()


@app.get("/health")
async def health():
    return {"status": "ok"}


@app.post("/mcp")
async def mcp_endpoint(request: Request):
    body = await request.json()
    method = body.get("method", "")
    req_id = body.get("id", 1)
    params = body.get("params", {})

    if method == "tools/list":
        return JSONResponse({"jsonrpc": "2.0", "result": {"tools": MCP_TOOLS}, "id": req_id})

    if method == "tools/call":
        tool_name = params.get("name", "")
        args = params.get("arguments", {})

        try:
            if tool_name == "summarize_url":
                text = await fetch_readable(args["url"])
                summary = await summarize_via_llm(text, args.get("max_length", 500))
                return JSONResponse({
                    "jsonrpc": "2.0",
                    "result": {"content": [{"type": "text", "text": summary}]},
                    "id": req_id,
                })

            elif tool_name == "summarize_text":
                summary = await summarize_via_llm(args["text"], args.get("max_length", 500))
                return JSONResponse({
                    "jsonrpc": "2.0",
                    "result": {"content": [{"type": "text", "text": summary}]},
                    "id": req_id,
                })

            else:
                return JSONResponse({
                    "jsonrpc": "2.0",
                    "error": {"code": -32601, "message": f"Unknown tool: {tool_name}"},
                    "id": req_id,
                })
        except Exception as e:
            return JSONResponse({
                "jsonrpc": "2.0",
                "error": {"code": -32000, "message": str(e)},
                "id": req_id,
            })

    return JSONResponse({
        "jsonrpc": "2.0",
        "error": {"code": -32601, "message": f"Unknown method: {method}"},
        "id": req_id,
    })

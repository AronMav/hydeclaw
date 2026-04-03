"""Generic stdio-to-HTTP bridge for MCP servers.

Reads MCP_COMMAND env var (JSON array) and spawns the process for each request.
Handles the MCP initialize handshake automatically.
"""

import asyncio
import json
import os

from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse

app = FastAPI()
COMMAND = json.loads(os.environ.get("MCP_COMMAND", '["echo","no MCP_COMMAND configured"]'))


async def stdio_call(method: str, params: dict, req_id):
    """Spawn MCP subprocess with initialize handshake, return matching response."""
    messages = [
        {"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "bridge", "version": "1.0"},
        }},
        {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
        {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params or {}},
    ]
    stdin_data = ("\n".join(json.dumps(m) for m in messages) + "\n").encode()

    proc = await asyncio.create_subprocess_exec(
        *COMMAND,
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        stdout, stderr = await asyncio.wait_for(proc.communicate(stdin_data), timeout=30)
    except asyncio.TimeoutError:
        proc.kill()
        await proc.wait()
        raise RuntimeError("MCP process timeout (30s)")

    for line in stdout.decode("utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
            if obj.get("id") == req_id:
                return obj
        except json.JSONDecodeError:
            pass

    err = stderr.decode("utf-8", errors="replace")[:500]
    raise RuntimeError(f"No valid JSON response from MCP. stderr: {err}")


@app.get("/health")
async def health():
    return {"ok": True}


@app.post("/mcp")
async def mcp_endpoint(request: Request):
    body = await request.json()
    try:
        result = await stdio_call(
            body.get("method", ""),
            body.get("params", {}),
            body.get("id", 1),
        )
        return JSONResponse(result)
    except Exception as e:
        return JSONResponse({
            "jsonrpc": "2.0",
            "error": {"code": -32000, "message": str(e)},
            "id": body.get("id", 1),
        })

"""Stock Analysis — MCP server for MOEX portfolio analysis."""

import os
import httpx
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse

app = FastAPI()
http_client: httpx.AsyncClient = None

MOEX_ISS_URL = "https://iss.moex.com/iss"


@app.on_event("startup")
async def startup():
    global http_client
    http_client = httpx.AsyncClient(timeout=30.0)


@app.on_event("shutdown")
async def shutdown():
    if http_client:
        await http_client.aclose()


MCP_TOOLS = [
    {
        "name": "get_stock_price",
        "description": "Get current price and today's change for a MOEX stock ticker.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "ticker": {"type": "string", "description": "MOEX ticker symbol (e.g. 'SBER', 'GAZP', 'LKOH')"},
            },
            "required": ["ticker"],
        },
    },
    {
        "name": "get_portfolio_summary",
        "description": "Get prices for multiple MOEX tickers at once. Returns current price and change for each.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "tickers": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of MOEX ticker symbols",
                },
            },
            "required": ["tickers"],
        },
    },
    {
        "name": "search_ticker",
        "description": "Search for a MOEX ticker by company name or partial ticker.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Company name or partial ticker to search"},
            },
            "required": ["query"],
        },
    },
]


async def get_stock_price(ticker: str) -> str:
    """Get stock price from MOEX ISS API."""
    ticker = ticker.upper()
    resp = await http_client.get(
        f"{MOEX_ISS_URL}/engines/stock/markets/shares/boards/TQBR/securities/{ticker}.json",
        params={"iss.meta": "off"},
    )
    resp.raise_for_status()
    data = resp.json()

    md = data.get("marketdata", {})
    cols = md.get("columns", [])
    rows = md.get("data", [])

    if not rows:
        return f"Тикер {ticker} не найден на MOEX (TQBR)."

    row = dict(zip(cols, rows[0]))
    last = row.get("LAST", row.get("LCURRENTPRICE"))
    change_pct = row.get("LASTTOPREVPRICE")
    open_price = row.get("OPEN")
    high = row.get("HIGH")
    low = row.get("LOW")
    volume = row.get("VALTODAY")

    sec = data.get("securities", {})
    sec_cols = sec.get("columns", [])
    sec_rows = sec.get("data", [])
    sec_row = dict(zip(sec_cols, sec_rows[0])) if sec_rows else {}
    name = sec_row.get("SHORTNAME", ticker)

    lines = [f"{name} ({ticker})"]
    if last is not None:
        lines.append(f"Цена: {last} ₽")
    if change_pct is not None:
        sign = "+" if change_pct > 0 else ""
        lines.append(f"Изменение: {sign}{change_pct:.2f}%")
    if open_price is not None:
        lines.append(f"Открытие: {open_price}")
    if high is not None and low is not None:
        lines.append(f"Диапазон: {low} — {high}")
    if volume is not None:
        lines.append(f"Объём: {volume:,.0f} ₽")

    return "\n".join(lines)


async def get_portfolio_summary(tickers: list[str]) -> str:
    """Get prices for multiple tickers."""
    results = []
    for ticker in tickers:
        try:
            result = await get_stock_price(ticker)
            results.append(result)
        except Exception as e:
            results.append(f"{ticker}: error — {e}")
    return "\n\n".join(results)


async def search_ticker(query: str) -> str:
    """Search MOEX for securities matching query."""
    resp = await http_client.get(
        f"{MOEX_ISS_URL}/securities.json",
        params={"q": query, "iss.meta": "off", "limit": 10},
    )
    resp.raise_for_status()
    data = resp.json()

    sec = data.get("securities", {})
    cols = sec.get("columns", [])
    rows = sec.get("data", [])

    if not rows:
        return f"Ничего не найдено по запросу '{query}'."

    lines = [f"Результаты поиска '{query}':"]
    for row_data in rows[:10]:
        row = dict(zip(cols, row_data))
        secid = row.get("secid", "?")
        name = row.get("shortname", row.get("name", "?"))
        market = row.get("group", "")
        lines.append(f"  {secid} — {name} ({market})")

    return "\n".join(lines)


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
            if tool_name == "get_stock_price":
                result = await get_stock_price(args["ticker"])
            elif tool_name == "get_portfolio_summary":
                result = await get_portfolio_summary(args["tickers"])
            elif tool_name == "search_ticker":
                result = await search_ticker(args["query"])
            else:
                return JSONResponse({
                    "jsonrpc": "2.0",
                    "error": {"code": -32601, "message": f"Unknown tool: {tool_name}"},
                    "id": req_id,
                })

            return JSONResponse({
                "jsonrpc": "2.0",
                "result": {"content": [{"type": "text", "text": result}]},
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

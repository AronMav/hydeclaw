"""Weather — MCP server for weather data via wttr.in."""

import os
import httpx
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse

app = FastAPI()
http_client: httpx.AsyncClient = None


@app.on_event("startup")
async def startup():
    global http_client
    http_client = httpx.AsyncClient(timeout=15.0)


@app.on_event("shutdown")
async def shutdown():
    if http_client:
        await http_client.aclose()


MCP_TOOLS = [
    {
        "name": "get_weather",
        "description": "Get current weather and forecast for a city. Returns detailed weather info in Russian.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "city": {"type": "string", "description": "City name (e.g. 'Samara', 'Moscow', 'London')"},
                "days": {"type": "integer", "default": 1, "description": "Forecast days (1-3)"},
            },
            "required": ["city"],
        },
    },
]


async def get_weather(city: str, days: int = 1) -> str:
    """Fetch weather from wttr.in."""
    days = max(1, min(3, days))
    resp = await http_client.get(
        f"https://wttr.in/{city}",
        params={"format": "j1", "lang": "ru"},
        headers={"Accept-Language": "ru"},
    )
    resp.raise_for_status()
    data = resp.json()

    current = data.get("current_condition", [{}])[0]
    location = data.get("nearest_area", [{}])[0]

    area = location.get("areaName", [{}])[0].get("value", city)
    country = location.get("country", [{}])[0].get("value", "")
    temp = current.get("temp_C", "?")
    feels = current.get("FeelsLikeC", "?")
    desc_list = current.get("lang_ru", current.get("weatherDesc", [{}]))
    desc = desc_list[0].get("value", "?") if desc_list else "?"
    humidity = current.get("humidity", "?")
    wind = current.get("windspeedKmph", "?")
    wind_dir = current.get("winddir16Point", "")
    pressure = current.get("pressure", "?")

    lines = [
        f"Weather: {area}, {country}",
        f"Now: {temp}°C (feels like {feels}°C), {desc}",
        f"Humidity: {humidity}%, Wind: {wind} km/h {wind_dir}",
        f"Pressure: {pressure} mb",
    ]

    forecasts = data.get("weather", [])[:days]
    for day in forecasts:
        date = day.get("date", "")
        max_t = day.get("maxtempC", "?")
        min_t = day.get("mintempC", "?")
        hourly = day.get("hourly", [])
        noon = hourly[4] if len(hourly) > 4 else hourly[0] if hourly else {}
        noon_desc_list = noon.get("lang_ru", noon.get("weatherDesc", [{}]))
        noon_desc = noon_desc_list[0].get("value", "?") if noon_desc_list else "?"
        lines.append(f"\n{date}: {min_t}..{max_t}°C, {noon_desc}")

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
            if tool_name == "get_weather":
                result = await get_weather(
                    city=args.get("city", ""),
                    days=args.get("days", 1),
                )
                return JSONResponse({
                    "jsonrpc": "2.0",
                    "result": {"content": [{"type": "text", "text": result}]},
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

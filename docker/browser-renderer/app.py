"""Browser Renderer — headless Chromium text extraction + automation service."""

import asyncio
import time
import uuid
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException
from fastapi.responses import Response
from pydantic import BaseModel, Field
from playwright.async_api import async_playwright, Browser, Page

browser: Browser | None = None
pw_instance = None

# ── Session management ────────────────────────────────────────────────────────
sessions: dict[str, Page] = {}
session_last_used: dict[str, float] = {}
SESSION_TTL = 300  # 5 minutes idle timeout
CLEANUP_INTERVAL = 30  # seconds


async def session_cleanup_task():
    """Background task to close idle sessions."""
    while True:
        await asyncio.sleep(CLEANUP_INTERVAL)
        now = time.time()
        expired = [
            sid for sid, last in session_last_used.items()
            if now - last > SESSION_TTL
        ]
        for sid in expired:
            page = sessions.pop(sid, None)
            session_last_used.pop(sid, None)
            if page:
                try:
                    await page.close()
                except Exception:
                    pass


def touch_session(sid: str):
    session_last_used[sid] = time.time()


def get_session_page(session_id: str) -> Page:
    if session_id not in sessions:
        raise HTTPException(404, f"Session {session_id} not found")
    touch_session(session_id)
    return sessions[session_id]


@asynccontextmanager
async def lifespan(app: FastAPI):
    global browser, pw_instance
    pw_instance = await async_playwright().start()
    browser = await pw_instance.chromium.launch(
        headless=True,
        args=["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage"],
    )
    cleanup = asyncio.create_task(session_cleanup_task())
    yield
    cleanup.cancel()
    # Close all sessions
    for page in sessions.values():
        try:
            await page.close()
        except Exception:
            pass
    sessions.clear()
    await browser.close()
    await pw_instance.stop()


app = FastAPI(title="Browser Renderer", lifespan=lifespan)

# ── Original endpoints (stateless) ───────────────────────────────────────────

class ExtractRequest(BaseModel):
    url: str
    timeout: int = Field(default=30, ge=1, le=60, description="Page load timeout in seconds")
    selector: str | None = Field(default=None, description="CSS selector to wait for before extracting")


class ExtractResponse(BaseModel):
    title: str
    description: str
    text: str
    url: str


STRIP_SELECTORS = [
    "script", "style", "noscript", "iframe", "svg",
    "nav", "header", "footer", "[role=navigation]",
    "[role=banner]", "[class*=cookie]", "[class*=popup]",
    "[class*=modal]", "[class*=sidebar]", "[class*=ad-]",
    "[class*=advertisement]", "[id*=ad-]",
]

CONTENT_SELECTORS = ["article", "main", "[role=main]", ".content", "#content", "body"]

DEFAULT_USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
DEFAULT_VIEWPORT = {"width": 1280, "height": 720}


@app.post("/extract", response_model=ExtractResponse)
async def extract(req: ExtractRequest):
    page = await browser.new_page(
        user_agent=DEFAULT_USER_AGENT,
    )
    try:
        await page.goto(req.url, wait_until="domcontentloaded", timeout=req.timeout * 1000)

        # Wait for JS rendering: custom selector or a short delay
        if req.selector:
            try:
                await page.wait_for_selector(req.selector, timeout=10000)
            except Exception:
                pass
        else:
            await page.wait_for_timeout(3000)

        # Extract metadata
        title = await page.title() or ""
        description = await page.evaluate(
            """() => {
                const el = document.querySelector('meta[name="description"]');
                return el ? el.getAttribute('content') || '' : '';
            }"""
        )

        # Strip noise elements
        for sel in STRIP_SELECTORS:
            await page.evaluate(
                f"document.querySelectorAll('{sel}').forEach(el => el.remove())"
            )

        # Extract main content
        text = ""
        for sel in CONTENT_SELECTORS:
            result = await page.evaluate(
                f"""() => {{
                    const el = document.querySelector('{sel}');
                    return el ? el.innerText : '';
                }}"""
            )
            cleaned = " ".join(result.split()) if result else ""
            if len(cleaned) > 100:
                text = cleaned
                break

        # Truncate to ~8000 chars
        if len(text) > 8000:
            text = text[:8000] + "..."

        return ExtractResponse(
            title=title.strip(),
            description=(description or "").strip(),
            text=text,
            url=req.url,
        )
    finally:
        await page.close()


class ScreenshotRequest(BaseModel):
    url: str
    timeout: int = Field(default=15, ge=1, le=60)
    full_page: bool = False


@app.post("/screenshot")
async def screenshot(req: ScreenshotRequest):
    page = await browser.new_page(
        viewport=DEFAULT_VIEWPORT,
        user_agent=DEFAULT_USER_AGENT,
    )
    try:
        await page.goto(req.url, wait_until="domcontentloaded", timeout=req.timeout * 1000)
        await page.wait_for_timeout(2000)
        img_bytes = await page.screenshot(full_page=req.full_page)
        media_type = "image/png"
        if len(img_bytes) > 10 * 1024 * 1024:  # Telegram limit ~10MB
            # Re-take as JPEG with quality reduction for large screenshots
            img_bytes = await page.screenshot(full_page=req.full_page, type="jpeg", quality=80)
            media_type = "image/jpeg"
        return Response(content=img_bytes, media_type=media_type)
    finally:
        await page.close()


# ── Automation endpoints (stateful sessions) ─────────────────────────────────

class AutomationRequest(BaseModel):
    action: str
    session_id: str | None = None
    url: str | None = None
    selector: str | None = None
    text: str | None = None
    js: str | None = None
    timeout: int = Field(default=10, ge=1, le=60)
    fields: dict | None = None
    full_page: bool = False


@app.post("/automation")
async def automation(req: AutomationRequest):
    """Unified browser automation endpoint. Dispatches by `action` field."""
    action = req.action

    # ── create_session ────────────────────────────────────────────────────
    if action == "create_session":
        sid = str(uuid.uuid4())[:8]
        page = await browser.new_page(
            viewport=DEFAULT_VIEWPORT,
            user_agent=DEFAULT_USER_AGENT,
        )
        sessions[sid] = page
        touch_session(sid)
        return {"session_id": sid, "status": "created"}

    # All other actions require session_id
    if not req.session_id:
        raise HTTPException(400, "session_id is required for this action")

    page = get_session_page(req.session_id)

    # ── navigate ──────────────────────────────────────────────────────────
    if action == "navigate":
        if not req.url:
            raise HTTPException(400, "url is required")
        await page.goto(req.url, wait_until="domcontentloaded", timeout=req.timeout * 1000)
        title = await page.title() or ""
        return {"status": "navigated", "url": req.url, "title": title}

    # ── click ─────────────────────────────────────────────────────────────
    if action == "click":
        if not req.selector:
            raise HTTPException(400, "selector is required")
        await page.click(req.selector, timeout=req.timeout * 1000)
        return {"status": "clicked", "selector": req.selector}

    # ── type ──────────────────────────────────────────────────────────────
    if action == "type":
        if not req.selector or req.text is None:
            raise HTTPException(400, "selector and text are required")
        await page.fill(req.selector, req.text)
        return {"status": "typed", "selector": req.selector}

    # ── fill (multiple fields) ────────────────────────────────────────────
    if action == "fill":
        if not req.fields:
            raise HTTPException(400, "fields dict is required")
        for sel, val in req.fields.items():
            await page.fill(sel, str(val))
        return {"status": "filled", "fields_count": len(req.fields)}

    # ── screenshot ────────────────────────────────────────────────────────
    if action == "screenshot":
        png_bytes = await page.screenshot(full_page=req.full_page)
        return Response(content=png_bytes, media_type="image/png")

    # ── wait ──────────────────────────────────────────────────────────────
    if action == "wait":
        if not req.selector:
            raise HTTPException(400, "selector is required")
        await page.wait_for_selector(req.selector, timeout=req.timeout * 1000)
        return {"status": "found", "selector": req.selector}

    # ── text ──────────────────────────────────────────────────────────────
    if action == "text":
        if req.selector:
            el = await page.query_selector(req.selector)
            if not el:
                return {"text": "", "error": f"Selector '{req.selector}' not found"}
            text = await el.inner_text()
        else:
            text = await page.inner_text("body")
        # Truncate
        if len(text) > 8000:
            text = text[:8000] + "..."
        return {"text": text}

    # ── evaluate ──────────────────────────────────────────────────────────
    if action == "evaluate":
        if not req.js:
            raise HTTPException(400, "js is required")
        result = await page.evaluate(req.js)
        return {"result": result}

    # ── content (full HTML + text) ────────────────────────────────────────
    if action == "content":
        html = await page.content()
        text = await page.inner_text("body")
        if len(html) > 50000:
            html = html[:50000] + "..."
        if len(text) > 8000:
            text = text[:8000] + "..."
        return {"html": html, "text": text, "url": page.url}

    # ── close ─────────────────────────────────────────────────────────────
    if action == "close":
        sessions.pop(req.session_id, None)
        session_last_used.pop(req.session_id, None)
        await page.close()
        return {"status": "closed", "session_id": req.session_id}

    raise HTTPException(400, f"Unknown action: {action}")


@app.get("/health")
async def health():
    return {"status": "ok", "sessions": len(sessions)}

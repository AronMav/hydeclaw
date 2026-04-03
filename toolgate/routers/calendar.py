"""Google Calendar integration via Service Account."""

import logging
import os
from datetime import datetime, timedelta

from fastapi import APIRouter
from pydantic import BaseModel

log = logging.getLogger("toolgate.calendar")
router = APIRouter(prefix="/calendar", tags=["calendar"])

SCOPES = ["https://www.googleapis.com/auth/calendar"]
SERVICE_ACCOUNT_FILE = os.environ.get("GOOGLE_SA_KEY", "config/google-service-account.json")
CALENDAR_ID = os.environ.get("GOOGLE_CALENDAR_ID", "primary")


def _get_service():
    """Build Google Calendar API service (lazy, no caching — called per-request)."""
    try:
        from google.oauth2 import service_account
        from googleapiclient.discovery import build
    except ImportError:
        raise RuntimeError("google-api-python-client and google-auth not installed. Run: pip install google-api-python-client google-auth")

    if not os.path.exists(SERVICE_ACCOUNT_FILE):
        raise FileNotFoundError(f"Service account key not found: {SERVICE_ACCOUNT_FILE}")

    creds = service_account.Credentials.from_service_account_file(
        SERVICE_ACCOUNT_FILE, scopes=SCOPES
    )
    return build("calendar", "v3", credentials=creds, cache_discovery=False)


class CreateEventRequest(BaseModel):
    summary: str
    start: str  # ISO format: 2026-03-22T10:00:00
    end: str    # ISO format: 2026-03-22T11:00:00
    description: str = ""
    location: str = ""
    timezone: str = "UTC"


@router.get("/today")
async def today_events():
    """Get today's events."""
    try:
        service = _get_service()
    except Exception as e:
        return {"error": str(e), "events": []}

    now = datetime.utcnow()
    start = now.replace(hour=0, minute=0, second=0, microsecond=0).isoformat() + "Z"
    end = now.replace(hour=23, minute=59, second=59, microsecond=0).isoformat() + "Z"

    try:
        result = service.events().list(
            calendarId=CALENDAR_ID, timeMin=start, timeMax=end,
            singleEvents=True, orderBy="startTime"
        ).execute()
    except Exception as e:
        log.error("Calendar API error: %s", e)
        return {"error": str(e), "events": []}

    events = []
    for e in result.get("items", []):
        events.append({
            "summary": e.get("summary", "(no subject)"),
            "start": e["start"].get("dateTime", e["start"].get("date", "")),
            "end": e["end"].get("dateTime", e["end"].get("date", "")),
            "location": e.get("location", ""),
            "description": (e.get("description") or "")[:200],
        })
    return {"events": events, "date": now.strftime("%Y-%m-%d")}


@router.get("/upcoming")
async def upcoming_events(days: int = 7, limit: int = 20):
    """Get upcoming events for next N days."""
    try:
        service = _get_service()
    except Exception as e:
        return {"error": str(e), "events": []}

    now = datetime.utcnow()
    start = now.isoformat() + "Z"
    end_dt = now + timedelta(days=days)
    end = end_dt.isoformat() + "Z"

    try:
        result = service.events().list(
            calendarId=CALENDAR_ID, timeMin=start, timeMax=end,
            singleEvents=True, orderBy="startTime", maxResults=limit
        ).execute()
    except Exception as e:
        log.error("Calendar API error: %s", e)
        return {"error": str(e), "events": []}

    events = []
    for e in result.get("items", []):
        events.append({
            "summary": e.get("summary", "(no subject)"),
            "start": e["start"].get("dateTime", e["start"].get("date", "")),
            "end": e["end"].get("dateTime", e["end"].get("date", "")),
            "location": e.get("location", ""),
        })
    return {"events": events, "days": days}


@router.post("/create")
async def create_event(req: CreateEventRequest):
    """Create a new calendar event."""
    try:
        service = _get_service()
    except Exception as e:
        return {"error": str(e)}

    event_body = {
        "summary": req.summary,
        "start": {"dateTime": req.start, "timeZone": req.timezone},
        "end": {"dateTime": req.end, "timeZone": req.timezone},
    }
    if req.description:
        event_body["description"] = req.description
    if req.location:
        event_body["location"] = req.location

    try:
        created = service.events().insert(calendarId=CALENDAR_ID, body=event_body).execute()
    except Exception as e:
        log.error("Calendar create error: %s", e)
        return {"error": str(e)}

    return {
        "status": "created",
        "id": created.get("id", ""),
        "link": created.get("htmlLink", ""),
        "summary": req.summary,
        "start": req.start,
        "end": req.end,
    }

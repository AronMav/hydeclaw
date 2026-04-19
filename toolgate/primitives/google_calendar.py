"""Google Calendar primitive — stateless list and create via service account.

The full service account JSON is passed in the request body (as a string),
eliminating the current toolgate dependency on a file-on-disk.
"""

import json
import logging
from datetime import datetime, timedelta, timezone
from typing import Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

try:
    from google.oauth2 import service_account
    from googleapiclient.discovery import build
    _GOOGLE_AVAILABLE = True
except ImportError:
    _GOOGLE_AVAILABLE = False

log = logging.getLogger("toolgate.primitives.google_calendar")
router = APIRouter(prefix="/primitives/google_calendar", tags=["primitives"])

SCOPES = ["https://www.googleapis.com/auth/calendar"]


class EventsListRequest(BaseModel):
    sa_key_json: str
    calendar_id: str = "primary"
    time_min: Optional[str] = None
    time_max: Optional[str] = None
    max_results: int = 20


class EventsCreateRequest(BaseModel):
    sa_key_json: str
    calendar_id: str = "primary"
    summary: str
    start: str
    end: str
    description: Optional[str] = None
    location: Optional[str] = None
    timezone: str = "UTC"


def _build_service(sa_key_json: str):
    """Parse SA key JSON, build a Calendar v3 service client."""
    if not _GOOGLE_AVAILABLE:
        raise HTTPException(500, "google-api-python-client and google-auth not installed")
    try:
        info = json.loads(sa_key_json)
    except json.JSONDecodeError as e:
        raise HTTPException(400, f"sa_key_json is not valid JSON: {e}") from e
    try:
        creds = service_account.Credentials.from_service_account_info(info, scopes=SCOPES)
    except ValueError as e:
        raise HTTPException(400, f"sa_key_json missing required fields: {e}") from e
    return build("calendar", "v3", credentials=creds, cache_discovery=False)


@router.post("/events/list")
async def events_list(req: EventsListRequest):
    """List Google Calendar events in a time window (default: next 7 days)."""
    service = _build_service(req.sa_key_json)

    # Default window: next 7 days from now if neither bound is set
    if req.time_min is None and req.time_max is None:
        now = datetime.now(timezone.utc)
        time_min = now.isoformat().replace("+00:00", "Z")
        time_max = (now + timedelta(days=7)).isoformat().replace("+00:00", "Z")
    else:
        time_min = req.time_min
        time_max = req.time_max

    try:
        result = service.events().list(
            calendarId=req.calendar_id,
            timeMin=time_min,
            timeMax=time_max,
            maxResults=req.max_results,
            singleEvents=True,
            orderBy="startTime",
        ).execute()
    except Exception as e:
        # Log exception type only; googleapiclient may include creds in str(e).
        log.warning("Google Calendar list failed: type=%s", type(e).__name__)
        raise HTTPException(502, f"Google Calendar API error: {type(e).__name__}") from e

    events = []
    for item in result.get("items", []):
        events.append({
            "id": item.get("id"),
            "summary": item.get("summary", ""),
            "start": item.get("start", {}),
            "end": item.get("end", {}),
            "location": item.get("location"),
            "description": item.get("description"),
            "html_link": item.get("htmlLink"),
        })

    return {"events": events}


@router.post("/events/create")
async def events_create(req: EventsCreateRequest):
    """Create a Google Calendar event."""
    service = _build_service(req.sa_key_json)

    body = {
        "summary": req.summary,
        "start": {"dateTime": req.start, "timeZone": req.timezone},
        "end": {"dateTime": req.end, "timeZone": req.timezone},
    }
    if req.description:
        body["description"] = req.description
    if req.location:
        body["location"] = req.location

    try:
        result = service.events().insert(calendarId=req.calendar_id, body=body).execute()
    except Exception as e:
        log.warning("Google Calendar insert failed: type=%s", type(e).__name__)
        raise HTTPException(502, f"Google Calendar API error: {type(e).__name__}") from e

    return {"event": {
        "id": result.get("id"),
        "summary": result.get("summary"),
        "html_link": result.get("htmlLink"),
    }}

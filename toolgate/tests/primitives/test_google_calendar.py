"""Unit tests for /primitives/google_calendar/*.

Requires google-api-python-client + google-auth installed. If they're absent
(e.g. a minimal dev env), the whole module is skipped rather than failing
with AttributeError when patching unresolved module attributes.
"""

import pytest

pytest.importorskip("googleapiclient")
pytest.importorskip("google.oauth2")

from unittest.mock import MagicMock, patch

from fastapi.testclient import TestClient


SA_KEY_SAMPLE = '{"type":"service_account","project_id":"p","private_key":"k","client_email":"sa@p.iam.gserviceaccount.com","token_uri":"https://oauth2.googleapis.com/token"}'


@pytest.fixture
def client():
    from fastapi import FastAPI
    from primitives import google_calendar

    app = FastAPI()
    app.include_router(google_calendar.router)
    return TestClient(app)


def test_events_list_rejects_missing_fields(client):
    resp = client.post("/primitives/google_calendar/events/list", json={})
    assert resp.status_code == 422


@patch("primitives.google_calendar.build")
@patch("primitives.google_calendar.service_account.Credentials")
def test_events_list_happy_path(mock_creds_cls, mock_build, client):
    mock_service = MagicMock()
    mock_build.return_value = mock_service
    mock_service.events.return_value.list.return_value.execute.return_value = {
        "items": [
            {"summary": "Standup", "start": {"dateTime": "2026-04-20T10:00:00Z"}, "end": {"dateTime": "2026-04-20T10:30:00Z"}}
        ]
    }

    resp = client.post("/primitives/google_calendar/events/list", json={
        "sa_key_json": SA_KEY_SAMPLE,
        "calendar_id": "primary",
        "time_min": "2026-04-20T00:00:00Z",
        "max_results": 20,
    })

    assert resp.status_code == 200, resp.text
    data = resp.json()
    assert "events" in data
    assert len(data["events"]) == 1
    assert data["events"][0]["summary"] == "Standup"

    mock_creds_cls.from_service_account_info.assert_called_once()


@patch("primitives.google_calendar.build")
@patch("primitives.google_calendar.service_account.Credentials")
def test_events_create_happy_path(mock_creds_cls, mock_build, client):
    mock_service = MagicMock()
    mock_build.return_value = mock_service
    mock_service.events.return_value.insert.return_value.execute.return_value = {
        "id": "evt123",
        "summary": "Meet",
        "htmlLink": "https://calendar.google.com/event?eid=...",
    }

    resp = client.post("/primitives/google_calendar/events/create", json={
        "sa_key_json": SA_KEY_SAMPLE,
        "calendar_id": "primary",
        "summary": "Meet",
        "start": "2026-04-21T10:00:00",
        "end": "2026-04-21T11:00:00",
        "timezone": "UTC",
    })

    assert resp.status_code == 200, resp.text
    data = resp.json()
    assert data["event"]["id"] == "evt123"


def test_events_list_invalid_sa_key_json_returns_400(client):
    resp = client.post("/primitives/google_calendar/events/list", json={
        "sa_key_json": "not json",
        "calendar_id": "primary",
    })
    assert resp.status_code == 400
    # FastAPI HTTPException puts message under "detail"
    assert "json" in resp.json()["detail"].lower()

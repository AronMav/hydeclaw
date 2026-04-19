"""Unit tests for /primitives/imap/* endpoints."""

from unittest.mock import MagicMock, patch

import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def client():
    """TestClient over a FastAPI app that mounts only the IMAP primitive router."""
    from fastapi import FastAPI
    from primitives import imap

    app = FastAPI()
    app.include_router(imap.router)
    return TestClient(app)


def test_imap_fetch_rejects_missing_fields(client):
    """pydantic validation — missing user/password/server/etc. → 422."""
    resp = client.post("/primitives/imap/fetch", json={})
    assert resp.status_code == 422


@patch("primitives.imap.imaplib.IMAP4_SSL")
def test_imap_fetch_happy_path(mock_imap_cls, client):
    """Valid request → imaplib invoked with correct credentials, messages returned."""
    mock_imap = MagicMock()
    mock_imap_cls.return_value = mock_imap
    mock_imap.select.return_value = ("OK", [b"1"])
    mock_imap.search.return_value = ("OK", [b"42"])
    mock_imap.fetch.return_value = ("OK", [(
        b"42 (RFC822 {123}",
        b"From: sender@test.com\r\nSubject: hi\r\nDate: Mon, 1 Apr 2026 10:00:00 +0000\r\n\r\nbody text"
    )])
    mock_imap.close.return_value = ("OK", [])
    mock_imap.logout.return_value = ("BYE", [])

    resp = client.post("/primitives/imap/fetch", json={
        "server": "imap.test.com",
        "port": 993,
        "user": "me@test.com",
        "password": "secret",
        "folder": "INBOX",
        "limit": 10,
        "unread_only": False,
    })

    assert resp.status_code == 200, resp.text
    data = resp.json()
    assert "messages" in data
    assert len(data["messages"]) == 1
    assert data["messages"][0]["subject"] == "hi"
    assert data["messages"][0]["from"] == "sender@test.com"

    mock_imap_cls.assert_called_once_with("imap.test.com", 993)
    mock_imap.login.assert_called_once_with("me@test.com", "secret")


@patch("primitives.imap.imaplib.IMAP4_SSL")
def test_imap_fetch_auth_failure_returns_401(mock_imap_cls, client):
    """Login raises IMAP4.error → endpoint returns 401."""
    import imaplib
    mock_imap = MagicMock()
    mock_imap_cls.return_value = mock_imap
    mock_imap.login.side_effect = imaplib.IMAP4.error("auth failed")

    resp = client.post("/primitives/imap/fetch", json={
        "server": "imap.test.com", "port": 993,
        "user": "me@test.com", "password": "wrong",
    })
    assert resp.status_code == 401
    # FastAPI HTTPException puts message under "detail"
    assert "auth" in resp.json()["detail"].lower()


@patch("primitives.imap.imaplib.IMAP4_SSL")
def test_imap_search_happy_path(mock_imap_cls, client):
    mock_imap = MagicMock()
    mock_imap_cls.return_value = mock_imap
    mock_imap.select.return_value = ("OK", [b"1"])
    mock_imap.search.return_value = ("OK", [b"7 11"])
    mock_imap.fetch.return_value = ("OK", [(
        b"11 (RFC822 {80}",
        b"From: a@b.com\r\nSubject: match\r\nDate: Mon, 1 Apr 2026 10:00:00 +0000\r\n\r\nbody"
    )])
    mock_imap.close.return_value = ("OK", [])
    mock_imap.logout.return_value = ("BYE", [])

    resp = client.post("/primitives/imap/search", json={
        "server": "imap.test.com", "port": 993,
        "user": "me@test.com", "password": "secret",
        "query": "invoice", "limit": 5,
    })

    assert resp.status_code == 200, resp.text
    data = resp.json()
    assert data["count"] >= 1
    mock_imap.search.assert_called_with(None, 'TEXT "invoice"')


@patch("primitives.imap.imaplib.IMAP4_SSL")
def test_imap_fetch_folder_not_found_returns_404(mock_imap_cls, client):
    """imap.select() returning NO surfaces as 404, not 200 with empty results."""
    mock_imap = MagicMock()
    mock_imap_cls.return_value = mock_imap
    mock_imap.select.return_value = ("NO", [b"folder does not exist"])
    mock_imap.close.return_value = ("OK", [])
    mock_imap.logout.return_value = ("BYE", [])

    resp = client.post("/primitives/imap/fetch", json={
        "server": "imap.test.com", "port": 993,
        "user": "me@test.com", "password": "secret",
        "folder": "DoesNotExist",
    })
    assert resp.status_code == 404
    assert "folder" in resp.json()["detail"].lower()


@patch("primitives.imap.imaplib.IMAP4_SSL")
def test_imap_search_escapes_backslash_and_quote(mock_imap_cls, client):
    """IMAP TEXT search must escape both backslash and double-quote per RFC 3501."""
    mock_imap = MagicMock()
    mock_imap_cls.return_value = mock_imap
    mock_imap.select.return_value = ("OK", [b"1"])
    mock_imap.search.return_value = ("OK", [b""])
    mock_imap.close.return_value = ("OK", [])
    mock_imap.logout.return_value = ("BYE", [])

    resp = client.post("/primitives/imap/search", json={
        "server": "imap.test.com", "port": 993,
        "user": "me@test.com", "password": "secret",
        "query": r'foo\bar"baz',
        "limit": 5,
    })

    assert resp.status_code == 200, resp.text
    # Backslash escaped first, then quote: foo\bar"baz → foo\\bar\"baz
    expected_criteria = 'TEXT "foo\\\\bar\\"baz"'
    mock_imap.search.assert_called_with(None, expected_criteria)

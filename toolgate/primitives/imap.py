"""IMAP primitive — stateless fetch and search over IMAP.

Credentials are passed in the request body. No environment reads, no secret lookup.
"""

import email as email_lib
import email.header
import imaplib
import logging
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from typing import Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

log = logging.getLogger("toolgate.primitives.imap")
router = APIRouter(prefix="/primitives/imap", tags=["primitives"])


class ImapFetchRequest(BaseModel):
    server: str
    port: int = 993
    user: str
    password: str
    folder: str = "INBOX"
    limit: int = 10
    unread_only: bool = True
    since_days: Optional[int] = None


class ImapSearchRequest(BaseModel):
    server: str
    port: int = 993
    user: str
    password: str
    folder: str = "INBOX"
    query: str
    limit: int = 10


def _decode_header(raw) -> str:
    """Decode a RFC 2047 encoded header to a Python str."""
    if raw is None:
        return ""
    parts = email.header.decode_header(raw)
    out = []
    for value, charset in parts:
        if isinstance(value, bytes):
            try:
                out.append(value.decode(charset or "utf-8", errors="replace"))
            except (LookupError, TypeError):
                out.append(value.decode("utf-8", errors="replace"))
        else:
            out.append(value)
    return "".join(out)


def _get_text_body(msg) -> str:
    """Extract a plain-text body snippet from a parsed email.Message."""
    if msg.is_multipart():
        for part in msg.walk():
            if part.get_content_type() == "text/plain":
                payload = part.get_payload(decode=True)
                if payload:
                    charset = part.get_content_charset() or "utf-8"
                    return payload.decode(charset, errors="replace")
        return ""
    payload = msg.get_payload(decode=True)
    if not payload:
        return ""
    charset = msg.get_content_charset() or "utf-8"
    return payload.decode(charset, errors="replace")


def _parse_uid(imap: imaplib.IMAP4_SSL, uid, snippet_len: int = 300) -> dict:
    """Fetch and parse one message by UID. Accepts bytes or str UID."""
    uid_str = uid.decode() if isinstance(uid, bytes) else str(uid)
    status, data = imap.fetch(uid, "(RFC822)")
    if status != "OK" or not data or not data[0]:
        return {"uid": uid_str, "error": "failed to fetch"}
    raw = data[0][1]
    msg = email_lib.message_from_bytes(raw)
    return {
        "uid": uid_str,
        "from": _decode_header(msg.get("From", "")),
        "subject": _decode_header(msg.get("Subject", "")),
        "date": _decode_header(msg.get("Date", "")),
        "snippet": _get_text_body(msg)[:snippet_len],
    }


@contextmanager
def _imap_session(req):
    """Open IMAP connection, login, select folder. Cleans up on exit.

    The ``req`` object must expose ``server``, ``port``, ``user``, ``password``,
    and ``folder`` attributes (duck-typed; used with both ImapFetchRequest and
    ImapSearchRequest).

    Raises HTTPException(502) on connect failure, (401) on auth failure,
    (404) on folder-not-found.
    """
    try:
        imap = imaplib.IMAP4_SSL(req.server, req.port)
    except OSError as e:
        raise HTTPException(502, f"IMAP connection failed: {e}") from e

    try:
        try:
            imap.login(req.user, req.password)
        except imaplib.IMAP4.error as e:
            raise HTTPException(401, f"IMAP auth failed: {e}") from e

        status, _ = imap.select(req.folder)
        if status != "OK":
            raise HTTPException(404, f"IMAP folder not found: {req.folder}")

        yield imap
    finally:
        try:
            imap.close()
        except Exception:
            pass
        try:
            imap.logout()
        except Exception:
            pass


@router.post("/fetch")
async def fetch(req: ImapFetchRequest):
    """Fetch recent messages from an IMAP folder."""
    with _imap_session(req) as imap:
        criteria_parts = []
        if req.unread_only:
            criteria_parts.append("UNSEEN")
        if req.since_days is not None:
            since_date = (datetime.now(timezone.utc) - timedelta(days=req.since_days)).strftime("%d-%b-%Y")
            criteria_parts.append(f'SINCE "{since_date}"')
        criteria = " ".join(criteria_parts) or "ALL"

        status, data = imap.search(None, criteria)
        if status != "OK":
            raise HTTPException(502, f"IMAP search failed: {status}")

        uids = data[0].split() if data and data[0] else []
        uids = uids[-req.limit:]  # most-recent N
        messages = [_parse_uid(imap, uid) for uid in reversed(uids)]

        return {"messages": messages, "count": len(messages)}


@router.post("/search")
async def search(req: ImapSearchRequest):
    """Full-text search an IMAP folder."""
    with _imap_session(req) as imap:
        # RFC 3501 IMAP quoted-string: escape backslash first, then double quote.
        safe_query = req.query.replace('\\', '\\\\').replace('"', '\\"')
        status, data = imap.search(None, f'TEXT "{safe_query}"')
        if status != "OK":
            raise HTTPException(502, f"IMAP search failed: {status}")

        uids = data[0].split() if data and data[0] else []
        uids = uids[-req.limit:]
        messages = [_parse_uid(imap, uid) for uid in reversed(uids)]

        return {"messages": messages, "count": len(messages)}

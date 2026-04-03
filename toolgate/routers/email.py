"""Email endpoints — IMAP inbox/search + SMTP send."""

import imaplib
import smtplib
import os
import email as email_lib
import email.header
import email.utils
from email.mime.text import MIMEText
from email.mime.multipart import MIMEMultipart
from typing import Optional, List

from fastapi import APIRouter
from fastapi.responses import JSONResponse
from pydantic import BaseModel

router = APIRouter(prefix="/email", tags=["email"])

IMAP_HOST = os.environ.get("EMAIL_IMAP_HOST", "imap.gmail.com")
IMAP_PORT = int(os.environ.get("EMAIL_IMAP_PORT", "993"))
SMTP_HOST = os.environ.get("EMAIL_SMTP_HOST", "smtp.gmail.com")
SMTP_PORT = int(os.environ.get("EMAIL_SMTP_PORT", "587"))
EMAIL_USER = os.environ.get("EMAIL_USER", "")
EMAIL_PASS = os.environ.get("EMAIL_PASS", "")


def _decode_header_value(raw) -> str:
    """Decode RFC2047-encoded email header value to a plain string."""
    if raw is None:
        return ""
    parts = email.header.decode_header(raw)
    decoded = []
    for fragment, charset in parts:
        if isinstance(fragment, bytes):
            try:
                decoded.append(fragment.decode(charset or "utf-8", errors="replace"))
            except (LookupError, UnicodeDecodeError):
                decoded.append(fragment.decode("utf-8", errors="replace"))
        else:
            decoded.append(str(fragment))
    return "".join(decoded)


def _get_text_body(msg) -> str:
    """Extract plain-text body from an email.Message object."""
    if msg.is_multipart():
        for part in msg.walk():
            ct = part.get_content_type()
            disp = str(part.get("Content-Disposition", ""))
            if ct == "text/plain" and "attachment" not in disp:
                payload = part.get_payload(decode=True)
                if payload:
                    charset = part.get_content_charset() or "utf-8"
                    try:
                        return payload.decode(charset, errors="replace")
                    except (LookupError, UnicodeDecodeError):
                        return payload.decode("utf-8", errors="replace")
    else:
        payload = msg.get_payload(decode=True)
        if payload:
            charset = msg.get_content_charset() or "utf-8"
            try:
                return payload.decode(charset, errors="replace")
            except (LookupError, UnicodeDecodeError):
                return payload.decode("utf-8", errors="replace")
    return ""


def _connect_imap() -> imaplib.IMAP4_SSL:
    """Open and authenticate an IMAP SSL connection."""
    if not EMAIL_USER or not EMAIL_PASS:
        raise ValueError("EMAIL_USER and EMAIL_PASS environment variables are not set")
    imap = imaplib.IMAP4_SSL(IMAP_HOST, IMAP_PORT)
    imap.login(EMAIL_USER, EMAIL_PASS)
    return imap


def _parse_message(imap: imaplib.IMAP4_SSL, uid: bytes, snippet_len: int = 300) -> dict:
    """Fetch and parse a single message by UID. Returns a message dict."""
    status, data = imap.uid("fetch", uid, "(RFC822)")
    if status != "OK" or not data or data[0] is None:
        return {}
    raw = data[0][1]
    msg = email_lib.message_from_bytes(raw)
    snippet = _get_text_body(msg)[:snippet_len].replace("\n", " ").strip()
    return {
        "id": uid.decode(),
        "from": _decode_header_value(msg.get("From", "")),
        "subject": _decode_header_value(msg.get("Subject", "")),
        "date": _decode_header_value(msg.get("Date", "")),
        "snippet": snippet,
    }


@router.get("/inbox")
async def inbox(
    folder: str = "INBOX",
    limit: int = 10,
    unread_only: bool = True,
):
    """Return recent messages from the specified folder."""
    imap = None
    try:
        imap = _connect_imap()
        imap.select(folder)
        criterion = "UNSEEN" if unread_only else "ALL"
        status, uid_list = imap.uid("search", None, criterion)
        if status != "OK":
            return JSONResponse(status_code=502, content={"error": "IMAP search failed"})

        uids = uid_list[0].split() if uid_list[0] else []
        # Newest first: reverse the list, take up to `limit`
        uids = list(reversed(uids))[:limit]

        messages = []
        for uid in uids:
            parsed = _parse_message(imap, uid)
            if parsed:
                messages.append(parsed)

        return {"messages": messages, "total": len(messages)}
    except ValueError as e:
        return JSONResponse(status_code=400, content={"error": str(e)})
    except imaplib.IMAP4.error as e:
        return JSONResponse(status_code=502, content={"error": f"IMAP error: {e}"})
    except Exception as e:
        return JSONResponse(status_code=500, content={"error": f"Unexpected error: {e}"})
    finally:
        if imap:
            try:
                imap.logout()
            except Exception:
                pass


@router.get("/search")
async def search(
    query: str,
    folder: str = "INBOX",
    limit: int = 10,
):
    """Search emails by subject or body text."""
    imap = None
    try:
        imap = _connect_imap()
        imap.select(folder)

        # IMAP TEXT searches both headers and body; use charset UTF-8 for Cyrillic queries
        try:
            status, uid_list = imap.uid("search", "CHARSET", "UTF-8", "TEXT", query.encode("utf-8"))
        except imaplib.IMAP4.error:
            # Some servers don't support CHARSET — fall back to ASCII-safe search
            safe_query = query.encode("ascii", errors="ignore").decode("ascii") or "ALL"
            status, uid_list = imap.uid("search", None, f'TEXT "{safe_query}"')

        if status != "OK":
            return JSONResponse(status_code=502, content={"error": "IMAP search failed"})

        uids = uid_list[0].split() if uid_list[0] else []
        uids = list(reversed(uids))[:limit]

        messages = []
        for uid in uids:
            parsed = _parse_message(imap, uid)
            if parsed:
                # Omit snippet for search results — keep response light
                messages.append({
                    "id": parsed["id"],
                    "from": parsed["from"],
                    "subject": parsed["subject"],
                    "date": parsed["date"],
                })

        return {"messages": messages, "query": query}
    except ValueError as e:
        return JSONResponse(status_code=400, content={"error": str(e)})
    except imaplib.IMAP4.error as e:
        return JSONResponse(status_code=502, content={"error": f"IMAP error: {e}"})
    except Exception as e:
        return JSONResponse(status_code=500, content={"error": f"Unexpected error: {e}"})
    finally:
        if imap:
            try:
                imap.logout()
            except Exception:
                pass


class SendRequest(BaseModel):
    to: str
    subject: str
    body: str
    html: bool = False


@router.post("/send")
async def send(req: SendRequest):
    """Send an email via SMTP."""
    try:
        if not EMAIL_USER or not EMAIL_PASS:
            return JSONResponse(
                status_code=400,
                content={"error": "EMAIL_USER and EMAIL_PASS environment variables are not set"},
            )

        if req.html:
            msg = MIMEMultipart("alternative")
            msg.attach(MIMEText(req.body, "html", "utf-8"))
        else:
            msg = MIMEText(req.body, "plain", "utf-8")

        msg["From"] = EMAIL_USER
        msg["To"] = req.to
        msg["Subject"] = req.subject

        with smtplib.SMTP(SMTP_HOST, SMTP_PORT) as smtp:
            smtp.ehlo()
            smtp.starttls()
            smtp.login(EMAIL_USER, EMAIL_PASS)
            smtp.sendmail(EMAIL_USER, [req.to], msg.as_string())

        return {"status": "sent", "to": req.to, "subject": req.subject}
    except smtplib.SMTPAuthenticationError as e:
        return JSONResponse(status_code=401, content={"error": f"SMTP authentication failed: {e}"})
    except smtplib.SMTPException as e:
        return JSONResponse(status_code=502, content={"error": f"SMTP error: {e}"})
    except Exception as e:
        return JSONResponse(status_code=500, content={"error": f"Unexpected error: {e}"})

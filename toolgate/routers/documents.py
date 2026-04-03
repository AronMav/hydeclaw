"""Document text extraction endpoint."""

from fastapi import APIRouter, Request
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from typing import Optional

from helpers import download_limited

router = APIRouter(tags=["documents"])


class ExtractTextUrlRequest(BaseModel):
    document_url: str
    max_chars: Optional[int] = 8000


@router.post("/extract-text-url")
async def extract_text_url(body: ExtractTextUrlRequest, request: Request):
    http = request.app.state.http_client

    try:
        doc_bytes, raw_ct = await download_limited(http, body.document_url)
    except Exception as e:
        if hasattr(e, 'status_code') and e.status_code == 413:
            raise
        return JSONResponse(status_code=502, content={"error": f"Failed to download document: {e}"})

    content_type = raw_ct.split(";")[0].strip().lower()
    filename = body.document_url.split("/")[-1].split("?")[0].lower()

    text = ""
    try:
        if content_type == "application/pdf" or filename.endswith(".pdf"):
            import fitz
            doc = fitz.open(stream=doc_bytes, filetype="pdf")
            pages = [page.get_text() for page in doc]
            doc.close()
            text = "\n\n".join(pages)

        elif content_type in (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/msword",
        ) or filename.endswith(".docx"):
            import docx
            import io
            document = docx.Document(io.BytesIO(doc_bytes))
            text = "\n".join(p.text for p in document.paragraphs)

        elif (
            "text" in content_type
            or content_type == "application/json"
            or filename.endswith((".txt", ".md", ".csv", ".json", ".log", ".xml", ".html"))
        ):
            text = doc_bytes.decode("utf-8", errors="replace")

        else:
            try:
                text = doc_bytes.decode("utf-8")
            except UnicodeDecodeError:
                return JSONResponse(status_code=415,
                                    content={"error": f"Unsupported document format: {content_type or filename}"})
    except Exception as e:
        return JSONResponse(status_code=500, content={"error": f"Failed to extract text: {e}"})

    max_chars = body.max_chars or 8000
    if len(text) > max_chars:
        text = text[:max_chars] + f"\n...[truncated, {len(text)} total chars]"

    return {"text": text.strip()}

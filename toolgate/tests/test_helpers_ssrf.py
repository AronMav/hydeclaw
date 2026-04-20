"""Regression tests for toolgate SSRF guard — Bug 1 (redirects) + Bug 2 (CGNAT/multicast).

These tests pin the contract uncovered by audit of a3a2f79..38ee92f:

  Bug 1 (score 95) — `download_limited` passes `follow_redirects=True` to httpx.
  A malicious origin can 302 to a private-IP target AFTER the pre-flight
  `validate_url_ssrf` check has already passed. Mirror Rust
  `ssrf_http_client` (commit 75fee11) which uses redirect policy NONE.

  Bug 2 (score 85) — Python's `ipaddress` stdlib does NOT classify
  CGNAT 100.64.0.0/10 or IPv4 multicast 224.0.0.0/4 under is_private /
  is_loopback / is_link_local / is_reserved. Rust `ssrf.rs::is_private_ip`
  blocks both; bring the Python guard to parity.

If any future author reverts the fix, these tests go red.
"""

import socket

import httpx
import pytest
from fastapi import HTTPException

from helpers import download_limited, validate_url_ssrf


def _fake_getaddrinfo(ip: str):
    """Return a socket.getaddrinfo replacement that always resolves to `ip`."""
    def fake(host, port, family=0, type=0, proto=0, flags=0):
        family = socket.AF_INET6 if ":" in ip else socket.AF_INET
        return [(family, socket.SOCK_STREAM, 0, "", (ip, port or 0))]
    return fake


# ── Bug 1: download_limited must not follow redirects ──────────────────────

class TestDownloadLimitedNoRedirect:
    @pytest.mark.asyncio
    async def test_download_limited_does_not_follow_redirects(self, monkeypatch):
        """302 responses must NOT be silently followed; raise_for_status trips on 3xx."""
        # Bypass SSRF guard — we're testing httpx redirect policy, not DNS.
        monkeypatch.setattr("helpers.validate_url_ssrf", lambda url: None)

        seen_urls: list[str] = []

        def handler(request: httpx.Request) -> httpx.Response:
            seen_urls.append(str(request.url))
            if request.url.path == "/start":
                return httpx.Response(
                    302,
                    headers={"Location": "http://example.com/final"},
                )
            # /final — would be reached only if redirect is followed.
            return httpx.Response(200, content=b"FOLLOWED", headers={"content-type": "text/plain"})

        transport = httpx.MockTransport(handler)
        async with httpx.AsyncClient(transport=transport) as http:
            with pytest.raises(httpx.HTTPStatusError):
                await download_limited(http, "http://example.com/start")

        # Exactly one request: to /start only. /final must NOT be fetched.
        assert len(seen_urls) == 1, f"expected 1 request, got {seen_urls}"
        assert seen_urls[0].endswith("/start")
        assert not any(u.endswith("/final") for u in seen_urls)


# ── Bug 2a: CGNAT 100.64.0.0/10 must be blocked ────────────────────────────

class TestValidateUrlSsrfCgnat:
    def test_validate_url_ssrf_blocks_cgnat(self, monkeypatch):
        """100.64.1.2 (inside CGNAT) must raise HTTPException(400, 'blocked: ...')."""
        monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("100.64.1.2"))
        with pytest.raises(HTTPException) as exc_info:
            validate_url_ssrf("http://tailscale-peer.example.com/")
        assert exc_info.value.status_code == 400
        assert "blocked:" in exc_info.value.detail

    def test_validate_url_ssrf_blocks_cgnat_upper_bound(self, monkeypatch):
        """100.127.255.254 (top of 100.64.0.0/10) must be blocked."""
        monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("100.127.255.254"))
        with pytest.raises(HTTPException) as exc_info:
            validate_url_ssrf("http://cgnat-top.example.com/")
        assert exc_info.value.status_code == 400
        assert "blocked:" in exc_info.value.detail

    def test_validate_url_ssrf_allows_just_outside_cgnat(self, monkeypatch):
        """100.63.255.255 (one below /10) and 100.128.0.0 (one above /10) are public."""
        monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("100.63.255.255"))
        validate_url_ssrf("http://below-cgnat.example.com/")  # must not raise

        monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("100.128.0.0"))
        validate_url_ssrf("http://above-cgnat.example.com/")  # must not raise


# ── Bug 2b: IPv4 multicast 224.0.0.0/4 must be blocked ─────────────────────

class TestValidateUrlSsrfMulticast:
    def test_validate_url_ssrf_blocks_multicast_v4(self, monkeypatch):
        """239.255.255.250 (SSDP) must be blocked."""
        monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("239.255.255.250"))
        with pytest.raises(HTTPException) as exc_info:
            validate_url_ssrf("http://ssdp.example.com/")
        assert exc_info.value.status_code == 400
        assert "blocked:" in exc_info.value.detail

    def test_validate_url_ssrf_blocks_multicast_low(self, monkeypatch):
        """224.0.0.1 (all-hosts multicast) must be blocked."""
        monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("224.0.0.1"))
        with pytest.raises(HTTPException) as exc_info:
            validate_url_ssrf("http://mcast.example.com/")
        assert exc_info.value.status_code == 400
        assert "blocked:" in exc_info.value.detail


# ── Regression guards: existing behavior must not drift ────────────────────

class TestValidateUrlSsrfRegressionGuards:
    def test_validate_url_ssrf_blocks_rfc1918(self, monkeypatch):
        monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("10.0.0.1"))
        with pytest.raises(HTTPException) as exc_info:
            validate_url_ssrf("http://rfc1918.example.com/")
        assert exc_info.value.status_code == 400
        assert "blocked:" in exc_info.value.detail

    def test_validate_url_ssrf_blocks_loopback(self):
        """Hostname 'localhost' is caught by the blocklist before DNS resolution."""
        with pytest.raises(HTTPException) as exc_info:
            validate_url_ssrf("http://localhost/")
        assert exc_info.value.status_code == 400
        assert "blocked:" in exc_info.value.detail

    def test_validate_url_ssrf_allows_public(self, monkeypatch):
        """8.8.8.8 (Google DNS, a legitimate public address) must pass."""
        monkeypatch.setattr(socket, "getaddrinfo", _fake_getaddrinfo("8.8.8.8"))
        validate_url_ssrf("http://dns.google/")  # must not raise

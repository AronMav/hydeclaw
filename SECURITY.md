# Security

## Reporting Vulnerabilities

If you discover a security vulnerability, please report it privately via GitHub's [Security Advisories](https://github.com/AronMav/hydeclaw/security/advisories/new) feature. Do not open a public issue.

## Security Model

HydeClaw is designed to run on a private LAN or behind a reverse proxy. It is NOT hardened for direct exposure to the public internet without additional measures (TLS termination, firewall, VPN).

### Authentication

- **HTTP API** — all endpoints require `Authorization: Bearer <token>` except health, uploads, webhooks, and OAuth callbacks.
- **WebSocket** — one-time tickets (`?ticket=<uuid>`, 30s TTL, consumed on first use) to avoid exposing the static token in URLs.
- **Webhooks** — per-webhook Bearer token (generic) or HMAC-SHA256 signature verification (GitHub).
- **Auth rate limiter** — 10 failed attempts per IP triggers a 5-minute lockout.
- **Request rate limiter** — configurable per-minute limit per IP.
- **Constant-time comparison** — all token checks use `subtle::ConstantTimeEq`.

### Secrets Vault

- **Encryption** — ChaCha20-Poly1305 (AEAD) with a unique random 12-byte nonce per secret.
- **Master key** — 32-byte hex key in `HYDECLAW_MASTER_KEY` env var. Losing it destroys all stored secrets.
- **Scoping** — secrets are scoped per-agent `(name, scope)`. Resolution: agent scope -> global -> env fallback.
- **Audit** — revealing a secret via `?reveal=true` emits an audit log entry.
- **Channel credentials** — bot tokens are extracted from config JSON, stored encrypted in vault under channel UUID scope, and redacted from the database `config` column.

### SSRF Protection

YAML tool execution uses a hardened HTTP client (`ssrf_http_client`):
- DNS resolver blocks RFC 1918 private IPs, loopback, and link-local addresses at resolution time.
- URL scheme validation blocks `file://`, `ftp://`, and non-HTTP schemes.
- Path parameters are URL-encoded; body templates are JSON-escaped.

### Loopback Restrictions

Requests from `127.0.0.1` / `::1` are allowed without auth only for specific internal paths:
- `/health`, `/api/mcp/callback`, `/api/channels/notify`, `/api/media/upload`, `/uploads/*`, `/ws`

All other loopback requests (including `/api/secrets`, `/api/backup`) still require Bearer auth.

### Docker Access

Core connects to Docker via TCP (`tcp://127.0.0.1:2375`). The Docker TCP listener is configured by `setup.sh` to bind only to localhost.

### Container Restart Whitelist

The API can only restart whitelisted containers (browser-renderer, searxng, mcp-*). PostgreSQL is excluded.

### Webhook Auth Throttling

Per-webhook failure counter: 5 auth failures within 5 minutes locks the webhook for 10 minutes. Prevents brute-forcing webhook secrets.

### Security Headers

Applied globally: `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `X-XSS-Protection: 1; mode=block`, `Referrer-Policy: strict-origin-when-cross-origin`.

### Tool Name Validation

API handlers enforce `[a-zA-Z0-9_-]` on tool, MCP entry, and skill names to prevent path traversal.

### Code Execution Sandbox

The `code_exec` tool runs user code in an isolated Docker container with:
- No network access
- Read-only filesystem (except `/tmp`)
- Memory and CPU limits
- Execution timeout

## Best Practices for Deployment

1. **Always use TLS** — run behind nginx/Caddy with HTTPS, or use a VPN.
2. **Generate strong tokens** — `openssl rand -hex 32` for both `HYDECLAW_AUTH_TOKEN` and `HYDECLAW_MASTER_KEY`.
3. **Back up the master key** — store it separately from the database. Without it, all vault secrets are irrecoverable.
4. **Restrict network access** — bind to `127.0.0.1` if only local access is needed, or use firewall rules.
5. **Keep PostgreSQL local** — the default Docker config binds postgres to `127.0.0.1:5432`.
6. **Review tool definitions** — YAML tools can make arbitrary HTTP requests. Audit `workspace/tools/` before deployment.

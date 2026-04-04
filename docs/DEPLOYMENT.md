# Deployment Guide

## Quick Deploy (From Release)

```bash
tar xzf hydeclaw-v0.1.0.tar.gz
cd hydeclaw
./setup.sh
```

`setup.sh` handles everything: Docker, Bun, Python, PostgreSQL, systemd services, `.env` generation.

## Manual Deploy

### Prerequisites

- Linux (Debian/Ubuntu/Fedora), ARM64 or x86_64
- Docker 20+ with Docker Compose
- Bun 1.x
- Python 3.11+

### Step 1: Infrastructure

```bash
cd docker
docker compose up -d
```

This starts PostgreSQL 17 + pgvector, SearXNG, and browser-renderer.

### Step 2: Environment

Create `.env` in the hydeclaw directory:

```bash
HYDECLAW_AUTH_TOKEN=$(openssl rand -hex 32)
HYDECLAW_MASTER_KEY=$(openssl rand -hex 32)
DATABASE_URL=postgresql://hydeclaw:hydeclaw@localhost:5432/hydeclaw
```

> [!IMPORTANT]
> Back up `HYDECLAW_MASTER_KEY` securely. It encrypts the secrets vault. If lost, all stored secrets are unrecoverable.

### Step 3: Start Services

```bash
# Core (spawns channels + toolgate as child processes)
./hydeclaw-core-aarch64  # or x86_64

# Watchdog (optional, separate binary)
./hydeclaw-watchdog-aarch64

# Memory worker (optional, separate binary)
./hydeclaw-memory-worker-aarch64
```

### Step 4: Systemd (Production)

```ini
# ~/.config/systemd/user/hydeclaw-core.service
[Unit]
Description=HydeClaw Core
After=network.target

[Service]
Type=notify
WorkingDirectory=%h/hydeclaw
ExecStart=%h/hydeclaw/hydeclaw-core-aarch64
EnvironmentFile=%h/hydeclaw/.env
Restart=on-failure
WatchdogSec=30

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable --now hydeclaw-core
loginctl enable-linger $USER  # keep services running after logout
```

### Step 5: Verify

```bash
curl -sf -H "Authorization: Bearer $HYDECLAW_AUTH_TOKEN" http://localhost:18789/api/doctor | python3 -m json.tool
```

All checks should show `"ok": true`.

## Updating

```bash
~/hydeclaw/update.sh hydeclaw-v0.2.0.tar.gz
```

Preserves `.env`, `config/`, `workspace/`, and database. Restarts services automatically.

## Security Hardening

### Network

- Run behind a reverse proxy (nginx/caddy) with TLS
- Restrict port 18789 to localhost or trusted IPs
- Use firewall rules to block direct access from internet

### Authentication

- Use a strong random token (32+ bytes hex)
- Rotate `HYDECLAW_AUTH_TOKEN` periodically
- Consider IP allowlisting in reverse proxy

### Secrets

- Never commit `.env` to version control
- Back up `HYDECLAW_MASTER_KEY` separately from `.env`
- Use per-agent scoped secrets for API keys

### Sandbox

- Non-base agents execute code in Docker containers (isolated)
- Base agents (Hyde) run on host -- grant `base = true` only to trusted agents
- Credential directories (`.ssh`, `.aws`) are blocked from sandbox bind mounts
- Sensitive env vars (`HYDECLAW_*`, `DATABASE_URL`, `PIP_INDEX_URL`) are filtered

### CORS

Configure explicitly for production in `config/hydeclaw.toml`:

```toml
[gateway]
cors_origins = ["https://your-domain.com"]
```

## Troubleshooting

### Core won't start

```bash
journalctl --user -u hydeclaw-core -f --no-pager
```

Common causes:
- PostgreSQL not running: `docker ps | grep postgres`
- Wrong DATABASE_URL in `.env`
- Port 18789 already in use
- Missing `.env` file

### Channels not connecting

```bash
curl -H "Authorization: Bearer $TOKEN" http://localhost:18789/api/doctor
```

Check `channels.ok`. If false:
- Bun not installed: `bun --version`
- channels/ directory missing
- WebSocket connection refused (check core logs)

### Memory/embeddings not working

```bash
curl -H "Authorization: Bearer $TOKEN" http://localhost:18789/api/doctor
```

Check `toolgate.providers.embedding`. If null:
- Ollama not running or not configured
- Toolgate crashed: check `toolgate` in core logs
- Wrong embedding provider in active providers

### Agent not responding

1. Check session status: `GET /api/sessions?agent=AgentName`
2. Check for stuck `run_status: "running"` sessions
3. Restart: `POST /api/services/channels/restart`
4. Check provider connectivity: `GET /api/providers`

### High memory usage

- Normal idle: ~40-80 MB for core
- Check Docker containers: `docker stats`
- Check for runaway subagents: `GET /api/tasks`
- Memory worker stuck: `systemctl --user restart hydeclaw-memory-worker`

### Backup/Restore

```bash
# Create backup
curl -X POST -H "Authorization: Bearer $TOKEN" http://localhost:18789/api/backups

# List backups
curl -H "Authorization: Bearer $TOKEN" http://localhost:18789/api/backups

# Restore
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -F "file=@backup-2026-04-04.sql.gz" \
  http://localhost:18789/api/backups/restore
```

## Architecture Overview

```
systemd: hydeclaw-core (Rust)
  ├── child: channels (Bun/TypeScript) — Telegram, Discord, etc.
  ├── child: toolgate (Python/FastAPI) — STT, TTS, Vision, Embeddings
  └── connects to:
      ├── PostgreSQL 17 + pgvector (Docker)
      ├── SearXNG (Docker)
      ├── browser-renderer (Docker)
      └── LLM providers (HTTPS)

systemd: hydeclaw-watchdog (Rust) — health monitoring
systemd: hydeclaw-memory-worker (Rust) — background embedding tasks
```

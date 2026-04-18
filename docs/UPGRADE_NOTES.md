# HydeClaw Upgrade Notes

## Upgrading to v0.20+: toolgate config → Core API single source of truth

**Breaking change:** toolgate no longer reads the following environment variables.
Pre-create equivalent providers via the admin UI (or `POST /api/providers`)
**before** restarting hydeclaw-core, or toolgate will start in **degraded mode**
and capability endpoints will return 503 until providers are configured.

### Removed environment variables

| Deprecated env var | Replacement (in Core provider registry) |
|---|---|
| `WHISPER_URL`, `OLLAMA_API_KEY` (for STT) | Create provider with `type=stt`, `driver=whisper-local`, `base_url=<your whisper URL>` |
| `VISION_URL`, `VISION_MODEL`, `OLLAMA_API_KEY` | Create provider with `type=vision`, `driver=ollama`, `base_url=<vision URL>`, `default_model=<model>` |
| `TTS_BACKEND_URL` | Create provider with `type=tts`, `driver=qwen3-tts`, `base_url=<your Qwen3-TTS URL>` |
| `MINIMAX_API_KEY` (normalize LLM) | Create provider with `type=text`, `provider_type=openai-compatible`, `base_url=<MiniMax URL>`, `api_key=<key>`; then reference its UUID in the TTS provider's `options.normalize_provider_id` |

### Verifying the migration

1. **Before upgrade:** on the current Pi, list env vars:
   ```bash
   systemctl --user show-environment | grep -E 'WHISPER|VISION|OLLAMA|TTS_BACKEND|MINIMAX'
   ```
2. **For each listed var:** create the equivalent provider via UI (Settings → Media Providers → Add Provider).
3. **For the MINIMAX normalize case:** note the UUID of the new `text` provider you create. In the TTS provider editor, set `options.normalize_provider_id = "<that UUID>"` and `options.normalize = true`.
4. **Upgrade:** `./update.sh hydeclaw-v0.20.0.tar.gz`
5. **Verify:**
   ```bash
   curl -s http://localhost:9011/health | jq .
   ```
   Expected: `"degraded": false`, all used capabilities `true` in the `capabilities` map.

### Rollback

If providers were not pre-created, you can:
1. Revert to previous binary (`~/hydeclaw/hydeclaw-core-aarch64.bak` if kept)
2. **or** create providers retroactively via UI — toolgate will auto-reload on the first matching `PUT /api/providers/{id}`.

### Architectural rationale

See `docs/superpowers/specs/2026-04-18-toolgate-config-sot-design.md` for full
design context (degraded mode, nested `normalize_provider_id`, etc.).

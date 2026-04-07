# CLI Providers: Seamless Integration Design

## Problem

Connecting CLI-based LLM providers (Gemini CLI, Claude CLI, Codex CLI) to HydeClaw requires manual debugging:

1. **Prompt argument mismatch** — Gemini CLI's `-p` takes a named argument, Claude's `-p` is a flag. Prompt was appended positionally, breaking Gemini.
2. **Base agent routed to Docker sandbox** — CLI ran in a container without API keys or host dependencies instead of on the host.
3. **API key only via `.env`** — no mechanism to pass secrets vault keys to CLI process env. Manual `.env` editing required.
4. **No validation at creation** — provider saves without checking if CLI exists, key works, or output parses.
5. **No model discovery** — `supports_model_listing: false`, users must know exact model IDs.
6. **Rigid hardcoded configs** — adding a new CLI provider requires Rust code changes and a new HydeClaw release.

## Design

### 1. Generic CLI Provider Architecture

CLI providers are described by **presets** (built-in defaults) with **DB overrides** — not separate Rust files per provider.

#### Preset Structure

```rust
struct CliPreset {
    id: &'static str,              // "gemini-cli"
    name: &'static str,            // "Gemini CLI"
    command: &'static str,         // "gemini"
    args: &'static [&'static str], // ["--output-format", "json"]
    prompt_arg: Option<&'static str>,       // Some("-p") for Gemini, None for positional
    model_arg: Option<&'static str>,        // Some("--model")
    system_prompt_arg: Option<&'static str>,// None for Gemini, Some("--append-system-prompt") for Claude
    system_prompt_when: SystemPromptWhen,   // First | Always | Never
    env_key: &'static str,         // "GEMINI_API_KEY"
    clear_env: &'static [&'static str],    // Env vars to remove before spawn (security)
    models_provider: &'static str, // "google" — delegates to existing discover_models backend
    default_models: &'static [&'static str], // hardcoded fallback model list
    session_mode: CliSessionMode,  // None for Gemini, Always for Claude
    session_arg: Option<&'static str>,
    resume_args: &'static [&'static str],
    resume_output: Option<CliOutputFormat>, // Separate output mode for resumed sessions
    max_prompt_arg_chars: usize,   // Auto-switch to stdin if prompt exceeds this (default 100_000)
    no_output_timeout_secs: u64,   // Watchdog: kill if no stdout for this long (default 60)
}

enum SystemPromptWhen { First, Always, Never }
```

#### Three Built-in Presets

| Preset | command | prompt_arg | env_key | clear_env | models_provider | default_models |
|--------|---------|-----------|---------|-----------|-----------------|----------------|
| `gemini-cli` | `gemini` | `Some("-p")` | `GEMINI_API_KEY` | `[]` | `google` | gemini-3.1-pro-preview, gemini-3-flash-preview, gemini-2.5-flash, gemini-2.5-pro |
| `claude-cli` | `claude` | `None` (positional) | `ANTHROPIC_API_KEY` | `["CLAUDE_CODE_OAUTH_TOKEN"]` | `anthropic` | claude-sonnet-4-6, claude-opus-4-6, claude-haiku-4-5 |
| `codex-cli` | `codex` | `None` (positional) | `OPENAI_API_KEY` | `[]` | `openai` | codex-mini, gpt-4.1, o4-mini |

All presets default to: `system_prompt_when: First`, `max_prompt_arg_chars: 100_000`, `no_output_timeout_secs: 60`.

#### Two-Level Override System

1. **Built-in preset** (Rust code) — defaults that work out of the box.
2. **DB override** (`providers.options` JSONB) — per-provider field overrides. Applied on top of preset.

Priority: `providers.options` fields > built-in preset defaults.

This allows agents and users to update CLI configs (new args, changed flags) without a HydeClaw release.

### 2. API Key from Vault to CLI Process

#### Flow

1. **UI:** "API Key" field shown when creating a CLI provider. On save → `POST /api/secrets` stores key in vault under `env_key` name (e.g., `GEMINI_API_KEY`), scope = global.
2. **Config:** `CliBackendConfig.env_key` stores the secret name (from preset).
3. **Runtime:** `CliLlmProvider` holds `Arc<SecretsManager>` (received at construction from `AppState`). Before each CLI invocation, it resolves the key via `secrets.resolve(env_key, "")` and passes it to `CliRunner::run()` in the env map.
4. **Execution:** `execute_on_host()` passes resolved secrets via `cmd.env(k, v)`. Parent env is still inherited — existing `.env` keys work as fallback.

#### Secrets Resolution Order

1. Vault (encrypted, scoped)
2. Parent process env (inherited from systemd `EnvironmentFile`)
3. Not found → CLI process runs without the key → auth error → health-check catches it

### 3. Health-Check at Provider Creation

#### Endpoint

`POST /api/providers/{id}/test-cli`

#### Steps (sequential, total timeout 30s)

1. **which** — `which <command>` to verify CLI is installed. Capture path.
2. **version** — `<command> --version` to get version string.
3. **test run** — `<command> <args> <model_arg> <model> <prompt_arg> "say hi"` with API key from vault in env. Timeout 30s.
4. **parse** — verify output is valid JSON and contains a response field.

#### Response

```json
{
  "cli_found": true,
  "cli_path": "/home/aronmav/.bun/bin/gemini",
  "cli_version": "0.36.0",
  "auth_ok": true,
  "response_ok": true,
  "response_time_ms": 1504,
  "error": null
}
```

#### Error Cases

| Condition | Response |
|-----------|----------|
| CLI not found | `cli_found: false`, error: "Install: npm install -g @google/gemini-cli" |
| CLI found, no API key | `auth_ok: false`, error: "Enter API Key above" |
| CLI found, key invalid | `auth_ok: false`, error: "API key rejected (401)" |
| CLI found, timeout | `response_ok: false`, error: "CLI timed out after 30s" |
| CLI found, bad JSON | `response_ok: false`, error: "CLI output is not valid JSON" |

#### Trigger Points

- "Test Connection" button in provider form (manual)
- Automatically on first save of a CLI provider
- After agent updates CLI config fields via skill

### 4. Setup Wizard: Auto-Detect CLI Tools

#### Changes to Step 1 (Requirements)

Add parallel checks for `which claude`, `which gemini`, `which codex` alongside Docker/PG/disk.

Response addition:
```json
{
  "cli_tools": [
    { "name": "gemini-cli", "status": "ok", "version": "0.36.0", "path": "/home/aronmav/.bun/bin/gemini" },
    { "name": "claude-cli", "status": "not_found" },
    { "name": "codex-cli", "status": "not_found" }
  ]
}
```

#### UI Changes

**Step 1:** Show detected CLI tools:
- `Gemini CLI v0.36.0` with green checkmark
- `Claude CLI — not installed` (dimmed)
- `Codex CLI — not installed` (dimmed)

Not-installed CLIs are informational, not blockers (they are optional).

**Step 2 (Provider selection):** Detected CLI providers appear first in the list with a "Detected" badge. Selecting one pre-fills the form with preset values. User enters API key → Test → Save.

Non-detected CLI providers remain available (lower in the list, no badge) for cases where the binary is in a non-standard PATH.

### 5. Model Discovery for CLI Providers

#### Runtime Fetch via Delegation

Each CLI preset has `models_provider` — the provider type whose existing `discover_models()` implementation handles the API call:

- `gemini-cli` → `models_provider: "google"` → `fetch_google_models()` using `GEMINI_API_KEY`
- `claude-cli` → `models_provider: "anthropic"` → `fetch_anthropic_models()` using `ANTHROPIC_API_KEY`
- `codex-cli` → `models_provider: "openai"` → `fetch_openai_models()` using `OPENAI_API_KEY`

The API key is resolved from vault using the same `env_key`.

#### Hardcoded Fallback

If runtime fetch fails (no key, API unreachable, timeout):

| Preset | Fallback Models |
|--------|----------------|
| `gemini-cli` | gemini-3.1-pro-preview, gemini-3-flash-preview, gemini-2.5-flash, gemini-2.5-pro |
| `claude-cli` | claude-sonnet-4-6, claude-opus-4-6, claude-haiku-4-5 |
| `codex-cli` | codex-mini, gpt-4.1, o4-mini |

#### ProviderTypeMeta Changes

```rust
struct ProviderTypeMeta {
    // ... existing fields ...
    models_provider: Option<&'static str>,   // NEW: delegate model listing
    default_models: &'static [&'static str], // NEW: hardcoded fallback
}
```

`supports_model_listing` becomes `true` for CLI providers.

### 6. Agent Config Override via Skill

#### Capability

Base agents can update CLI provider configs via the `provider-management` skill. Fields that can be overridden:

- `command` — validated via `which` check
- `args` — base arguments array
- `prompt_arg` — prompt flag
- `model_arg` — model selection flag
- `env_key` — secret name for API key

#### Constraints

- Only `base = true` agents can modify CLI configs
- `command` changes require passing `which` validation
- After any config change, health-check runs automatically
- Changes stored in `providers.options` JSONB (DB override layer)

#### API

`PATCH /api/providers/{id}` with `options` field containing overrides:

```json
{
  "options": {
    "prompt_arg": "--prompt",
    "args": ["--output-format", "json", "--no-color"]
  }
}
```

### 7. Execution Hardening (from OpenClaw analysis)

Features adopted from OpenClaw's CLI runner, adapted to HydeClaw's Rust architecture.

#### Auto-switch arg → stdin

If prompt exceeds `max_prompt_arg_chars` (default 100,000), automatically switch from arg to stdin input mode. Prevents OS argument length limits and shell escaping issues with large prompts.

In `execute_on_host()`:
- If `prompt.len() > max_prompt_arg_chars`: remove prompt from argv, pipe it via `cmd.stdin(Stdio::piped())` + `child.stdin.write_all()`
- Otherwise: append as positional arg (current behavior)

#### Environment sanitization (clearEnv)

Before spawning CLI process, remove env vars listed in `clear_env` from the child's environment. Prevents credential leakage between providers (e.g., `CLAUDE_CODE_OAUTH_TOKEN` leaking into a Gemini CLI process).

In `execute_on_host()`:
- For each key in `clear_env`: `cmd.env_remove(key)`
- Applied after `cmd.env(k, v)` (vault secrets injected first, then stale vars cleared)

#### Watchdog no-output timeout

Separate from the overall timeout (300s). Kills CLI if it produces no stdout for `no_output_timeout_secs` (default 60s). Catches hung processes faster than waiting for the full timeout.

Implementation: wrap `child.wait_with_output()` with a stdout activity monitor — read stdout in a loop, reset a timer on each chunk. If timer fires → kill child.

#### System prompt control (systemPromptWhen)

- `First` (default): system prompt only on the first message in a session (no session = always)
- `Always`: system prompt on every invocation
- `Never`: never send system prompt to CLI

Prevents bloated prompts on resumed sessions where the CLI already has context.

#### Session invalidation

CLI session ID is invalidated (cleared) when:
- System prompt hash changes (agent SOUL.md updated)
- API key changes (re-authenticated)

Stored as part of `CliRunner` internal state. On invalidation → next call uses fresh `args` instead of `resume_args`.

#### JSONL streaming (future-ready)

Add `CliOutputFormat::Jsonl` support. For CLI tools that output newline-delimited JSON events (e.g., Codex `--json`), parse incrementally and emit `StreamEvent::TextDelta` in real-time instead of buffering the entire response.

Not required for v0.5.0 launch (Gemini and Claude use buffered JSON), but the plumbing should be in place.

## Files Changed

### Backend (~7 files)

| File | Change |
|------|--------|
| `cli_backend.rs` | Add `env_key`, `clear_env`, `max_prompt_arg_chars`, `no_output_timeout_secs`, `system_prompt_when` to config. Replace `default_*_backend()` with `CLI_PRESETS` array. Add preset lookup with DB override merge. Implement auto-switch arg→stdin, env sanitization, no-output watchdog, session invalidation. |
| `providers.rs` | Update `ProviderTypeMeta` with `models_provider`, `default_models`. Update `create_provider()` to resolve preset + merge options. Set `supports_model_listing: true` for CLI types. |
| `providers_claude_cli.rs` | Add `Arc<SecretsManager>` to `CliLlmProvider`. Resolve vault key before each `runner.run()` call. |
| `providers_gemini_cli.rs` | Remove (one-line re-export, no longer needed). |
| `model_discovery.rs` | In CLI provider match arms, delegate to `models_provider` backend. Add fallback to `default_models` on error. |
| `handlers/monitoring.rs` | Add `which` checks for CLI tools in `api_setup_requirements`. |
| `handlers/providers.rs` (or agents.rs) | Add `POST /api/providers/{id}/test-cli` endpoint. |

### Frontend (~3 files)

| File | Change |
|------|--------|
| Setup wizard (step 1) | Show detected CLI tools from requirements response. |
| Setup wizard (step 2) | Sort detected CLI providers first with "Detected" badge. Pre-fill form from preset. |
| Provider form | Add "API Key" field for CLI types. Add "Test Connection" button calling test-cli endpoint. Show result inline. |

## What Gets Deleted

- `providers_gemini_cli.rs` — one-line re-export file, merged into generic path
- `default_claude_backend()` / `default_gemini_backend()` functions — replaced by `CLI_PRESETS` array

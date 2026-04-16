# HydeClaw Bug Audit Report

**Date:** 2026-04-16
**Method:** 6 parallel Opus agents, each focused on a specific area
**Scope:** Full codebase — frontend, backend, security, database, engine
**Verification:** Architectural review agent verified CRITICAL+HIGH bugs

## Verification Summary

| Bug | Original | Verdict |
| --- | --- | --- |
| BUG-001 (SSE approval mismatch) | CRITICAL | **CONFIRMED** |
| BUG-003 (Agent switch regression) | HIGH | **FALSE POSITIVE** — intentional behavior |
| BUG-004 (Rename misses participants) | HIGH | **CONFIRMED** |
| BUG-005 (fetch_recent no agent_id) | HIGH | **PARTIALLY TRUE** — single-user system, arguably intentional |
| BUG-009 (Temperature 0 → 1.0) | MEDIUM | **CONFIRMED** |
| BUG-017 (SSRF redirect bypass) | MEDIUM | **CONFIRMED** |
| BUG-018 (Rate limit bypass) | MEDIUM | **PARTIALLY TRUE** — auth lockout provides secondary defense |
| BUG-021 (load_messages all branches) | MEDIUM | **FALSE POSITIVE** — callers intentionally want full history |

---

## CRITICAL (1)

### BUG-001: SSE approval event type mismatch — approvals in chat completely broken

**Frontend:** `ui/src/stores/sse-events.ts:24-25` expects `"tool-approval-needed"` / `"tool-approval-resolved"`
**Backend:** `crates/hydeclaw-core/src/gateway/mod.rs:42-43` emits `"approval-needed"` / `"approval-resolved"`

Backend sends `{"type": "approval-needed", ...}`, frontend `parseSseEvent()` switch never matches, event silently dropped. ApprovalCard never renders in chat stream. Users must use WS toast notification as only approval path.

**Verdict:** NEEDS VERIFICATION — check actual event names in both files.

---

## HIGH (6)

### BUG-002: Duplicate user message flash in chat

**File:** `ui/src/app/(authenticated)/chat/ChatThread.tsx:905-909`

Race between optimistic live message (status: "confirmed") and React Query refetch bringing the same message from DB. Brief render frame shows both.

**Verdict:** NEEDS VERIFICATION — check if dedup logic handles "confirmed" status correctly.

### BUG-003: Agent switch regression — stale session list causes switch-back

**File:** `ui/src/app/(authenticated)/chat/page.tsx:314-324`

When user selects Agent B, sidebar still shows Agent A sessions (stale React Query cache). Clicking a stale session triggers `selectSession(session.id, session.agent_id)` which calls `setCurrentAgent("A")`, switching back.

**Verdict:** CONFIRMED by user report.

### BUG-004: Agent rename does not update `sessions.participants` array

**File:** `crates/hydeclaw-core/src/gateway/handlers/agents/crud.rs:480-526`

Rename transaction covers 20 tables but misses `sessions.participants` TEXT[]. After rename, participant matching fails for renamed agent in multi-agent sessions.

**Verdict:** NEEDS VERIFICATION — check if participants is actually used for routing.

### BUG-005: `fetch_recent()` has no agent_id filter — cross-agent memory leak

**File:** `crates/hydeclaw-core/src/db/memory_queries.rs:252-274`

Unlike `fetch_pinned`, `search_semantic`, `search_fts` which filter by agent_id, `fetch_recent()` returns chunks from ALL agents. Called from `MemoryStore::recent()`.

**Verdict:** NEEDS VERIFICATION — check who calls `recent()` and whether cross-agent is intentional (admin view).

### BUG-006: SIGHUP reload creates dual engines racing on workspace

**File:** `crates/hydeclaw-core/src/main.rs:899-903`

On SIGHUP, new engine replaces old in map, but old engine's in-flight SSE task still runs with separate LoopDetector, memory_md_lock. Both can write workspace files concurrently.

**Verdict:** NEEDS VERIFICATION — check if SIGHUP is actually used in production.

### BUG-007: `processing_session_id` overwritten by concurrent SSE calls

**File:** `crates/hydeclaw-core/src/agent/engine_sse.rs:56`

Single `Arc<Mutex<Option<Uuid>>>` per engine. Two SSE requests for same agent → second overwrites session_id. Tools using fallback `processing_session_id` get wrong session.

**Verdict:** NEEDS VERIFICATION — check if concurrent SSE for same agent is possible (rate limiting? UI guard?).

---

## MEDIUM (21)

### BUG-008: Memory search fires on every keystroke — no debounce

**File:** `ui/src/app/(authenticated)/memory/page.tsx:89-110,196-199`

Each keystroke triggers API call with server-side semantic search (embedding computation). Typing "hello world" = 11 embedding requests.

**Verdict:** LIKELY TRUE — standard frontend bug pattern.

### BUG-009: Temperature 0 silently converted to 1.0

**File:** `ui/src/app/(authenticated)/agents/page.tsx:171`

`parseFloat(f.temperature) || 1.0` — `parseFloat("0")` returns `0` which is falsy in JS, falls through to `1.0`.

**Verdict:** CONFIRMED — classic JS falsy trap. Fix: `parseFloat(f.temperature) ?? 1.0` or explicit NaN check.

### BUG-010: Config page sends NaN for non-numeric input

**File:** `ui/src/app/(authenticated)/config/page.tsx:117-120`

`Number(editMaxReqPerMin)` without NaN guard. Non-numeric text → NaN → serialized as null.

**Verdict:** LIKELY TRUE — but `<input type="number">` provides partial protection in browsers.

### BUG-011: DocumentFullView swallows API errors

**File:** `ui/src/app/(authenticated)/memory/page.tsx:27-31`

No `.catch()` on fetch promise. Error → loading=false, content=null, blank page, no error message.

**Verdict:** LIKELY TRUE.

### BUG-012: Memory pagination phantom "Next" button

**File:** `ui/src/app/(authenticated)/memory/page.tsx:99-100,296`

When total is exact multiple of page size, last page has exactly `limit` items → Next enabled → clicks to empty page.

**Verdict:** LIKELY TRUE — off-by-one pagination pattern.

### BUG-013: Agent name without client-side validation

**File:** `ui/src/app/(authenticated)/agents/AgentEditDialog.tsx:234-240`

No pattern/regex validation. Backend validates `[a-zA-Z0-9_-]` and rejects, but UX is poor (generic error after Save).

**Verdict:** TRUE but LOW impact (backend catches it).

### BUG-014: Multi-agent session state lost (empty sessionParticipants)

**File:** `ui/src/stores/chat-store.ts:76-92`

`sessionParticipants` populated via SSE event from React Query cache. If cache invalidated, participants empty → multi-agent branch never executes → clean switch loses session.

**Verdict:** NEEDS VERIFICATION — check actual data flow.

### BUG-015: `ensure()` returns non-Immer reference

**File:** `ui/src/stores/chat-store.ts:28-34`

`fresh` object returned is the original, not the Immer proxy. Subsequent mutations via `set()` don't affect the returned reference.

**Verdict:** LIKELY FALSE POSITIVE — callers use `update()` which properly drafts. The returned value is read-only in practice.

### BUG-016: `activeSessionIds` not cleared on SSE finish

**File:** `ui/src/stores/streaming-renderer.ts:680-690`

SSE finish doesn't call `markSessionInactive`. Depends on WS `agent_processing` event. If WS disconnects during finish → session stuck as "running".

**Verdict:** LIKELY TRUE — fragile cross-channel dependency.

### BUG-017: SSRF redirect bypass via 302 to literal IP

**File:** `crates/hydeclaw-core/src/tools/ssrf.rs:98-105`

`SsrfSafeResolver` validates DNS of initial hostname only. Redirect to `http://127.0.0.1/` bypasses DNS check (no lookup needed for IP literal). Default reqwest follows up to 10 redirects.

**Verdict:** LIKELY TRUE — well-known SSRF bypass pattern. Fix: `redirect(Policy::none())`.

### BUG-018: Rate limit bypass via any Authorization header

**File:** `crates/hydeclaw-core/src/gateway/middleware.rs:195-198`

`has_auth = req.headers().get("authorization").is_some()` — presence check, not validity. `Authorization: Bearer garbage` bypasses rate limiter. Auth validation happens later in pipeline.

**Verdict:** CONFIRMED — the check is clearly `is_some()` not `is_valid()`.

### BUG-019: Sandbox Docker container has network access to infrastructure

**File:** `crates/hydeclaw-core/src/containers/sandbox.rs:290`

`network_disabled: Some(false)`, joins `hydeclaw` network. Non-base agent's `code_exec` can reach postgres, toolgate, searxng directly.

**Verdict:** NEEDS VERIFICATION — check if this is intentional for tool access (some tools need network).

### BUG-020: Empty auth token bypasses gmail push endpoint

**File:** `crates/hydeclaw-core/src/gateway/handlers/email_triggers.rs:174-175`

`if !expected_token.is_empty()` guard → empty HYDECLAW_AUTH_TOKEN skips all auth. Endpoint is in PUBLIC_EXACT list.

**Verdict:** LIKELY TRUE but UNLIKELY scenario — `.env` auto-generates token on first run.

### BUG-021: `load_messages()` returns all branches in flat mode

**File:** `crates/hydeclaw-core/src/db/sessions.rs:231-263`

No branch filtering. `knowledge_extractor`, `engine_commands`, `compact_session` call `load_messages()` → get messages from abandoned branches → corrupted LLM context.

**Verdict:** NEEDS VERIFICATION — check if callers actually need trunk-only or all messages.

### BUG-022: `get_session()` omits `retry_count` column

**File:** `crates/hydeclaw-core/src/db/sessions.rs:733-736`

SELECT misses `retry_count`. `#[sqlx(default)]` → always 0. Any code reading retry_count from this query gets wrong value.

**Verdict:** LIKELY TRUE — but check if any code actually reads retry_count from get_session() result.

### BUG-023: `set_session_run_status()` blocks transition FROM 'done'

**File:** `crates/hydeclaw-core/src/db/sessions.rs:325`

`WHERE run_status IS DISTINCT FROM 'done'` prevents ANY update when status is 'done'. Blocks setting 'failed' after 'done'.

**Verdict:** NEEDS VERIFICATION — might be intentional (done = terminal state).

### BUG-024: LIKE pattern injection in `check_allowlist()`

**File:** `crates/hydeclaw-core/src/db/approvals.rs:117`

`$2 LIKE REPLACE(tool_pattern, '*', '%')` — tool name with `%` or `_` matches unexpected patterns.

**Verdict:** LIKELY TRUE but LOW practical impact — tool names are validated to `[a-zA-Z0-9_-]` by API handlers, and `_` is a valid LIKE wildcard matching itself.

### BUG-025: Notifications table has no retention cleanup

**File:** `migrations/008_notifications.sql`

No TTL cleanup unlike audit_events, session_events, outbound_queue. Grows indefinitely on Pi.

**Verdict:** TRUE — confirmed by comparing with other tables that have cleanup.

### BUG-026: `warm_up_detector()` documented in CLAUDE.md but not implemented

**File:** CLAUDE.md references `session_wal::warm_up_detector()`, grep returns 0 matches in src/

LoopDetector always starts fresh. After crash, same looping tool gets `break_threshold` more executions before detection.

**Verdict:** CONFIRMED — grep shows no implementation.

### BUG-027: `api_import_config` without config_write_lock

**File:** `crates/hydeclaw-core/src/gateway/handlers/config.rs:511`

Writes to `config/hydeclaw.toml` without acquiring `config_write_lock`. Concurrent PUT + POST can corrupt TOML.

**Verdict:** LIKELY TRUE — check if import endpoint exists and is reachable.

### BUG-028: Restored webhooks with empty secret bypass auth

**File:** `crates/hydeclaw-core/src/gateway/handlers/webhooks.rs:374-385`

`if let Some(ref expected) = wh.secret && !expected.is_empty()` — empty secret = no auth. Backup restore can insert `secret: null`.

**Verdict:** LIKELY TRUE — but requires backup with manipulated webhook data.

---

## LOW / LOW-MEDIUM (39)

### Engine (3)

- **BUG-029:** Approval timeout vs webhook race — approved tool gets "timed out" (`approval_manager.rs:210-277`)
- **BUG-030:** Loop detector bypass within parallel batch — `check_limits()` sees pre-execution state (`engine_parallel.rs:51-57`)
- **BUG-031:** Killed agent receives one last message via mpsc (`engine_agent_tool.rs:196-232`)

### Engine LOW (2)

- **BUG-032:** SSE `try_send` silently drops events for slow clients (`chat.rs:566`)
- **BUG-033:** Relaxed ordering on ARM64 for status/last_result visibility (`session_agent_pool.rs:264,290`)

### Security LOW (5)

- **BUG-034:** SQL format with hardcoded table names — fragile pattern (`agents/crud.rs:493`)
- **BUG-035:** `localhost:80` not blocked in SSRF validation (`ssrf.rs:16-27`)
- **BUG-036:** Partial credential mask leaks 8 characters (`channels.rs:439-457`)
- **BUG-037:** WS connection budget released on 101, not on close (`middleware.rs:173-184`)
- **BUG-038:** Setup endpoints bypass auth lockout recording (`middleware.rs:314-316`)

### Database LOW (6)

- **BUG-039:** Token overflow u32→i32 in usage_log (`usage.rs:23-24`)
- **BUG-040:** `tool_quality` penalty_score = lifetime average, not rolling window (`tool_quality.rs:117-139`)
- **BUG-041:** `skill_versions` race on generation number — no UNIQUE constraint (`skill_versions.rs:27-48`)
- **BUG-042:** `outbound_queue` dedup-then-insert race (`outbound.rs:20-49`)
- **BUG-043:** `pairing_codes` no FK → orphaned records (`migrations/006_pairing_codes.sql`)
- **BUG-044:** Missing composite index `(session_id, created_at)` on session_events (`migrations/013_session_wal.sql`)

### API LOW (11)

- **BUG-045:** `api_import_config` doesn't reload shared_config in memory (`config.rs:511`)
- **BUG-046:** `api_channel_ack` accepts arbitrary status string → unrecoverable state (`channels.rs:229-258`)
- **BUG-047:** V1 backup restore wipes all providers — no empty check (`backup.rs:437-439`)
- **BUG-048:** `api_fork_session` no ownership or running state check (`sessions.rs:519-561`)
- **BUG-049:** `api_delete_all_sessions` doesn't cleanup session pools → memory leak (`sessions.rs:296-336`)
- **BUG-050:** Delete messages + session without transaction (`sessions.rs:246-256`)
- **BUG-051:** Restore: failed agents → 200 OK (`backup.rs:437`)
- **BUG-052:** `api_run_cron` no concurrent execution check (`cron.rs:378-487`)
- **BUG-053:** `Instant::duration_since` can panic on race (`webhooks.rs:46`)
- **BUG-054:** Hardcoded `'russian'` FTS in search_messages (`sessions.rs:687`)
- **BUG-055:** Backup exports pre-migration credentials from config column (`backup.rs:746-757`)

### Frontend chat LOW (4)

- **BUG-056:** `saveLastSession` never clears session ID (`chat-persistence.ts:21-28`)
- **BUG-057:** `reconnectAttempt` always 0 — indicator shows wrong count (`streaming-renderer.ts:177-196`)
- **BUG-058:** `uiStateSaveTimers` leak on agent switch (`streaming-renderer.ts:83`)
- **BUG-059:** djb2 hash collision can suppress valid re-renders (`chat-reconciliation.ts:15-33`)

### Frontend UI LOW (6)

- **BUG-060:** Config accepts negative/zero rate limits (`config/page.tsx:265-309`)
- **BUG-061:** Multiple pages use useSearchParams without Suspense (`memory/page.tsx:74`, `chat/page.tsx:74`)
- **BUG-062:** Accessibility gaps — no aria-labels on memory/config pages
- **BUG-063:** No error boundaries on memory, agents, config, providers pages
- **BUG-064:** Memory delete stale closure on rapid deletes (`memory/page.tsx:112-122`)
- **BUG-065:** `/api/providers/{id}/resolve` returns unmasked API key (by design)

---

## Summary

| Severity | Count |
| --- | --- |
| CRITICAL | 1 |
| HIGH | 6 |
| MEDIUM | 21 |
| LOW | 37 |
| **TOTAL** | **65** |

| Area | Count |
| --- | --- |
| Frontend (chat-store) | 10 |
| Frontend (UI components) | 12 |
| Agent Engine | 8 |
| Security | 10 |
| Database / Data Integrity | 13 |
| API Handlers | 15 |
| **TOTAL** | **65** (3 removed as duplicates/not-bugs from original 67) |

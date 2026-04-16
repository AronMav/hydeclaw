# Sprint 1: Critical + Security Fixes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 1 critical SSE approval mismatch + 4 security vulnerabilities identified in the bug audit.

**Architecture:** Five independent point fixes. Each is a self-contained 1-2 file change. No architectural dependencies between tasks — can be done in any order.

**Tech Stack:** Rust (Axum, reqwest, bollard), TypeScript (Next.js SSE events)

---

## File Map

**Modify:**
- `crates/hydeclaw-core/src/gateway/mod.rs:42-43` — fix SSE event type constants
- `crates/hydeclaw-core/src/tools/ssrf.rs:98-104` — add redirect policy
- `crates/hydeclaw-core/src/gateway/middleware.rs:193-198` — fix rate limit bypass
- `crates/hydeclaw-core/src/gateway/handlers/email_triggers.rs:171-179` — fix empty token guard
- `crates/hydeclaw-core/src/containers/sandbox.rs:290,295` — disable network for non-base agents

---

## Task 1: Fix SSE approval event type mismatch (BUG-001, CRITICAL)

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/mod.rs:42-43`

The backend emits `"approval-needed"` / `"approval-resolved"` but the frontend expects `"tool-approval-needed"` / `"tool-approval-resolved"`. Fix the backend constants to match the frontend.

- [ ] **Step 1: Fix the event type constants**

In `crates/hydeclaw-core/src/gateway/mod.rs`, change lines 42-43:

```rust
// before
pub const APPROVAL_NEEDED: &str = "approval-needed";
pub const APPROVAL_RESOLVED: &str = "approval-resolved";

// after
pub const APPROVAL_NEEDED: &str = "tool-approval-needed";
pub const APPROVAL_RESOLVED: &str = "tool-approval-resolved";
```

- [ ] **Step 2: Verify no other backend code references the old strings directly**

Run: `grep -rn '"approval-needed"\|"approval-resolved"' crates/ --include="*.rs"`

Expected: only the two lines just changed. If other files reference these strings by value (not via the constant), fix them too.

- [ ] **Step 3: Verify frontend matches**

Run: `grep -rn "tool-approval-needed\|tool-approval-resolved" ui/src/ --include="*.ts" --include="*.tsx"`

Expected: matches in `sse-events.ts` and `streaming-renderer.ts` confirming they expect the new names.

- [ ] **Step 4: Run tests**

Run: `cargo test -p hydeclaw-core 2>&1 | tail -5`

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/mod.rs
git commit -m "fix(critical): SSE approval event types must match frontend expectations"
```

---

## Task 2: Fix SSRF redirect bypass (BUG-017, MEDIUM)

**Files:**
- Modify: `crates/hydeclaw-core/src/tools/ssrf.rs:98-104`

The SSRF-safe HTTP client follows redirects by default. A 302 to `http://127.0.0.1/` bypasses the DNS-level private IP filter because reqwest skips the custom resolver for IP literals.

- [ ] **Step 1: Add redirect policy to SSRF client**

In `crates/hydeclaw-core/src/tools/ssrf.rs`, modify `ssrf_safe_client()`:

```rust
// before
pub fn ssrf_safe_client(timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(std::time::Duration::from_secs(10))
        .dns_resolver(Arc::new(SsrfSafeResolver))
        .build()
        .expect("failed to build SSRF-safe HTTP client")
}

// after
pub fn ssrf_safe_client(timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(std::time::Duration::from_secs(10))
        .dns_resolver(Arc::new(SsrfSafeResolver))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build SSRF-safe HTTP client")
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p hydeclaw-core ssrf 2>&1 | tail -5`

Expected: all SSRF tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/tools/ssrf.rs
git commit -m "fix(security): disable redirects in SSRF-safe HTTP client"
```

---

## Task 3: Fix rate limit bypass via fake Authorization header (BUG-018, MEDIUM)

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/middleware.rs:193-198`

Any request with an `Authorization` header (even `Bearer garbage`) bypasses the request rate limiter. Fix: remove the early-return exemption. The 300 rpm default limit is generous enough for legitimate single-user usage.

- [ ] **Step 1: Remove the auth-header exemption**

In `crates/hydeclaw-core/src/gateway/middleware.rs`, delete lines 193-198:

```rust
// DELETE these lines:
    // Exempt authenticated requests — rate limiting only protects against unauthenticated abuse.
    // Authenticated users (single-user self-hosted) should never be rate limited.
    let has_auth = req.headers().get("authorization").is_some();
    if has_auth {
        return next.run(req).await;
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p hydeclaw-core 2>&1 | tail -5`

Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/middleware.rs
git commit -m "fix(security): remove rate limit bypass for requests with Authorization header"
```

---

## Task 4: Fix empty auth token bypasses gmail push endpoint (BUG-020, MEDIUM)

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/email_triggers.rs:171-179`

When `HYDECLAW_AUTH_TOKEN` is empty/unset, the gmail push endpoint skips authentication entirely. Fix: reject if token is empty (fail-closed).

- [ ] **Step 1: Fix the guard to fail-closed**

In `crates/hydeclaw-core/src/gateway/handlers/email_triggers.rs`, change lines 171-179:

```rust
// before
    let expected_token = std::env::var("HYDECLAW_AUTH_TOKEN").unwrap_or_default();
    let provided_token = params.get("token").map_or("", std::string::String::as_str);
    use subtle::ConstantTimeEq;
    if !expected_token.is_empty()
        && !bool::from(provided_token.as_bytes().ct_eq(expected_token.as_bytes()))
    {
        tracing::warn!("gmail push: rejected request with invalid token");
        return StatusCode::UNAUTHORIZED.into_response();
    }

// after
    let expected_token = std::env::var("HYDECLAW_AUTH_TOKEN").unwrap_or_default();
    let provided_token = params.get("token").map_or("", std::string::String::as_str);
    use subtle::ConstantTimeEq;
    if expected_token.is_empty()
        || !bool::from(provided_token.as_bytes().ct_eq(expected_token.as_bytes()))
    {
        tracing::warn!("gmail push: rejected request (missing or invalid token)");
        return StatusCode::UNAUTHORIZED.into_response();
    }
```

Key change: `!expected_token.is_empty() &&` becomes `expected_token.is_empty() ||`. Empty token = always reject.

- [ ] **Step 2: Run tests**

Run: `cargo test -p hydeclaw-core 2>&1 | tail -5`

Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/email_triggers.rs
git commit -m "fix(security): reject gmail push when auth token is empty (fail-closed)"
```

---

## Task 5: Restrict sandbox network access for non-base agents (BUG-019, MEDIUM)

**Files:**
- Modify: `crates/hydeclaw-core/src/containers/sandbox.rs`

Non-base agent sandboxes join the `hydeclaw` Docker network, giving `code_exec` access to postgres, toolgate, and all infrastructure. Fix: only base agents get network access; non-base agents run network-isolated.

- [ ] **Step 1: Read sandbox code to find where `is_base` is available**

Read `crates/hydeclaw-core/src/containers/sandbox.rs` to understand the `run_code` method signature. Check if the `base` flag is already passed or needs to be added.

Run: `grep -n "pub async fn run_code\|pub async fn execute\|fn create_container" crates/hydeclaw-core/src/containers/sandbox.rs`

Also check the call site in the engine:

Run: `grep -rn "sandbox.*run_code\|sandbox.*execute\|code_exec" crates/hydeclaw-core/src/agent/ --include="*.rs" | head -10`

- [ ] **Step 2: Add is_base parameter and modify container config**

If `run_code` does not already receive `is_base`, add it to the method signature and update the container config:

```rust
// In the container Config block, change:
// before
network_disabled: Some(false),
// ...
network_mode: Some("hydeclaw".to_string()),

// after
network_disabled: Some(!is_base),
network_mode: if is_base { Some("hydeclaw".to_string()) } else { None },
```

- [ ] **Step 3: Update call sites**

Find all callers:

Run: `grep -rn "sandbox" crates/hydeclaw-core/src/agent/engine --include="*.rs" | grep -i "run\|exec" | head -10`

Update each to pass `self.agent.base` as the `is_base` argument.

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test -p hydeclaw-core 2>&1 | tail -5`

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/containers/sandbox.rs
git commit -m "fix(security): disable network for non-base agent sandboxes"
```

---

## Verification

After all 5 tasks:

- [ ] **Full test suite**

Run: `cargo test 2>&1 | grep "test result"`

- [ ] **Clippy clean**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`

- [ ] **Deploy and verify on Pi**

Build: `cargo zigbuild --release --target aarch64-unknown-linux-gnu -p hydeclaw-core`

Deploy: stop services, scp binary, start services, run doctor check.

- [ ] **Manual test: SSE approval in browser**

Open UI, chat, send message that triggers tool approval. Verify ApprovalCard renders in the chat stream (not just WS toast).

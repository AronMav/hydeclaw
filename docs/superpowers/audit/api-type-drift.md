# API Type Drift Audit — Phase D Output

**Date:** 2026-04-20
**Purpose:** Map every UI-facing HTTP endpoint to its `api.ts` interface, classify serialization method, record drift. Input for phases C/B/A of the UI API Type Codegen programme.

## Methodology

Three parallel scans per handler:
1. **Handler inventory** — `ls crates/hydeclaw-core/src/gateway/handlers/` + each `pub(crate) fn routes() -> Router<AppState>`.
2. **Serialization classification** — `grep -n "json!\|Json(json!" handlers/<file>.rs` → hand-rolled; `grep -n "^#\[derive.*Serialize\]" handlers/<file>.rs db/<file>.rs` → typed; both → mixed.
3. **TS mapping** — match endpoint/shape against interfaces in [ui/src/types/api.ts](../../../ui/src/types/api.ts).

## Handler Inventory & Classification

| # | File | Endpoint | Handler fn | Serialization | Rust type | TS interface | Drift |
|---|---|---|---|---|---|---|---|

(populated by tasks 3-8)

## Metrics

- **Total endpoints:** N (populated by task 11)
- **Typed (`#[derive(Serialize)]`):** N₁ — phase C scope
- **Hand-rolled (`json!{}`):** N₂ — phase A scope (minus pilot B)
- **Mixed:** N₃ — treated as hand-rolled
- **Handlers with no TS interface (UI uses `unknown`):** N₄
- **TS interfaces with no backing handler (dead code):** N₅ — removed during phase A

## Drift Summary

(list of concrete drifts found, populated by task 10)

## Merge Gate Decision

(populated by task 12)
- Typed ratio: N₁/(N₁+N₂) = __%
- **Gate:** ≥20% typed threshold for C-first priority.
- **Decision:** __ (proceed to phase C | reorder to B-first)
- **Rationale:** __

import { describe, it, expect, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";
import { qk } from "@/lib/queries";

// ── Hoist the QueryClient so the vi.mock factory can reference it ─────────────
// vi.mock factories are moved to the top of the file by Vitest's transformer
// (before any `const` declarations), so local variables are not yet initialised
// at that point.  vi.hoisted() runs at the same time as the factory — it is the
// only safe place to create a value that both the factory and the test body share.
const { qc } = vi.hoisted(() => {
  const { QueryClient } = require("@tanstack/react-query");
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return { qc };
});

// ── Redirect the singleton queryClient used by getCachedHistoryMessages ───────
vi.mock("@/lib/query-client", () => ({ queryClient: qc }));

// ── Store mock ────────────────────────────────────────────────────────────────
// Follow the pattern from use-engine-running.test.tsx: expose a controlled
// state object and make useChatStore execute the selector against it.
vi.mock("@/stores/chat-store", () => {
  const state = {
    agents: {
      Arty: {
        messageSource: { mode: "history", sessionId: "sess-1" },
        activeSessionId: "sess-1",
        selectedBranches: {},
      },
    },
  };
  return {
    useChatStore: (selector: (s: typeof state) => unknown) => selector(state),
  };
});

// Import AFTER mocks are in place so the module resolves the mocked deps.
import { useRenderMessages } from "../use-render-messages";

// ── Wrapper ───────────────────────────────────────────────────────────────────
function wrapper({ children }: { children: React.ReactNode }) {
  return React.createElement(QueryClientProvider, { client: qc }, children);
}

describe("useRenderMessages — RQ cache subscription", () => {
  it("re-renders and surfaces messages when the RQ cache is populated after mount", async () => {
    const { result } = renderHook(() => useRenderMessages("Arty"), { wrapper });

    // Initially the cache is empty → hook must return [].
    expect(result.current.length).toBe(0);

    // Simulate ChatThread's useSessionMessages populating the cache.
    await act(async () => {
      qc.setQueryData(qk.sessionMessages("sess-1"), {
        messages: [
          {
            id: "m1",
            role: "user",
            content: "hello",
            tool_calls: null,
            tool_call_id: null,
            created_at: "2026-04-21T00:00:00Z",
            agent_id: null,
            feedback: null,
            edited_at: null,
            status: "done",
            thinking_blocks: null,
            parent_message_id: null,
            branch_from_message_id: null,
            abort_reason: null,
          },
          {
            id: "m2",
            role: "assistant",
            content: "hi",
            tool_calls: null,
            tool_call_id: null,
            created_at: "2026-04-21T00:00:01Z",
            agent_id: "Arty",
            feedback: null,
            edited_at: null,
            status: "done",
            thinking_blocks: null,
            parent_message_id: null,
            branch_from_message_id: null,
            abort_reason: null,
          },
        ],
      });
    });

    // After cache fill the hook MUST re-render and return the loaded messages.
    expect(result.current.length).toBeGreaterThan(0);
  });
});

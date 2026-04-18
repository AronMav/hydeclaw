/**
 * Regression tests for two bugs introduced while wiring UI Stop → backend cancel.
 *
 * Bug 1 (race): `startStream` used to call `abortActiveStream` for its own
 *   cleanup. That function now POSTs `/api/chat/{sid}/abort` to notify the
 *   backend — but the OLD stream on the same session id is about to be
 *   replaced by a NEW stream with the SAME session id. The backend's
 *   `stream_registry.cancel(sid)` then cancels the NEW stream's
 *   CancellationToken, so auto-scroll sees only ~1 partial chunk and then
 *   stops. Fix: `startStream` / `resumeStream` call the internal-only
 *   `abortLocalOnly` which skips the backend POST.
 *
 * Bug 2 (missing hook): user Stop button used to only call
 *   `ctrl.abort()` (client-side fetch drop). Backend engine kept running,
 *   streaming row stayed `status='streaming'` forever. Fix:
 *   `abortActiveStream` fires `POST /api/chat/{sid}/abort` which trips the
 *   backend CancellationToken → `LlmCallError::UserCancelled` →
 *   `persist_partial_if_any` writes the aborted row.
 *
 * These tests pin both contracts:
 *   - `abortActiveStream` MUST POST /api/chat/{sid}/abort when a controller
 *     exists and a sessionId is available.
 *   - `startStream`'s cleanup path MUST NOT POST /abort (would cancel the
 *     stream we are about to start on the same sid).
 */

import { describe, it, expect, vi, beforeEach } from "vitest";

const { mockApiPost, mockFetch } = vi.hoisted(() => ({
  mockApiPost: vi.fn().mockResolvedValue({}),
  mockFetch: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  apiPost: mockApiPost,
  apiPatch: vi.fn().mockResolvedValue({}),
  assertToken: () => "test-token",
  getToken: () => "test-token",
}));

vi.mock("@/lib/query-client", () => ({
  queryClient: {
    invalidateQueries: vi.fn(),
    setQueryData: vi.fn(),
    getQueryData: vi.fn(() => undefined),
  },
}));

vi.mock("@/lib/queries", () => ({
  qk: { sessionMessages: (sid: string) => ["session-messages", sid] },
}));

// Stub `fetch` so startStream doesn't try to open a real SSE connection.
// The mocked response body is an already-closed ReadableStream so the
// stream renderer's reader loop exits immediately.
beforeEach(() => {
  mockApiPost.mockClear();
  mockFetch.mockReset();
  mockFetch.mockImplementation(
    async () =>
      new Response(new ReadableStream({ start(c) { c.close(); } }), {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      }),
  );
  (globalThis as unknown as { fetch: typeof fetch }).fetch = mockFetch as unknown as typeof fetch;
});

import { createStreamingRenderer } from "@/stores/streaming-renderer";
import { emptyAgentState } from "@/stores/chat-types";

type Store = { agents: Record<string, ReturnType<typeof emptyAgentState>>; currentAgent: string };

/** Minimal store harness matching the StoreAccess interface. */
function makeStore(initial?: Partial<Store>): {
  get: () => Store;
  set: (fn: (draft: Store) => void) => void;
  snapshot: () => Store;
} {
  let state: Store = {
    currentAgent: "Arty",
    agents: { Arty: emptyAgentState() },
    ...initial,
  };
  return {
    get: () => state,
    set: (fn) => {
      const draft = JSON.parse(JSON.stringify(state));
      fn(draft);
      state = draft;
    },
    snapshot: () => state,
  };
}

describe("abortActiveStream contract", () => {
  it("POSTs /api/chat/{sid}/abort when controller + sessionId exist", () => {
    const store = makeStore({
      agents: {
        Arty: {
          ...emptyAgentState(),
          activeSessionId: "session-xyz",
          connectionPhase: "streaming",
        },
      },
    });
    const renderer = createStreamingRenderer(store);

    // Install a fake AbortController so the abortActiveStream branch fires.
    const ctrl = new AbortController();
    renderer.getAbortCtrl; // noop — just ensuring API exists
    // We have to call a startStream-like flow to get the controller set.
    // The renderer exposes no public setter, so drive it through startStream
    // and THEN call abortActiveStream. The mocked fetch closes immediately,
    // but the controller is stashed synchronously before the fetch promise.
    renderer.startStream("Arty", "session-xyz", [], "hi");
    // Clear any POSTs startStream may have made (should be 0, but defensive).
    mockApiPost.mockClear();

    renderer.abortActiveStream("Arty");

    expect(mockApiPost).toHaveBeenCalledTimes(1);
    expect(mockApiPost).toHaveBeenCalledWith("/api/chat/session-xyz/abort");
    // Silence unused-var lint.
    void ctrl;
  });

  it("does NOT POST /abort when there is no active controller (no-op)", () => {
    const store = makeStore({
      agents: {
        Arty: {
          ...emptyAgentState(),
          activeSessionId: "session-xyz",
          connectionPhase: "idle",
        },
      },
    });
    const renderer = createStreamingRenderer(store);

    // No prior startStream → no controller → no POST.
    renderer.abortActiveStream("Arty");

    expect(mockApiPost).not.toHaveBeenCalled();
  });

  it("does NOT POST /abort when activeSessionId is null (cannot target a session)", () => {
    const store = makeStore({
      agents: {
        Arty: {
          ...emptyAgentState(),
          activeSessionId: null,
          connectionPhase: "streaming",
        },
      },
    });
    const renderer = createStreamingRenderer(store);

    // Force an AbortController to exist by starting a stream, then re-read
    // activeSessionId. Because the mocked fetch closes synchronously, the
    // controller ends up present at the instant abortActiveStream runs.
    renderer.startStream("Arty", null, [], "hello");
    mockApiPost.mockClear();
    renderer.abortActiveStream("Arty");

    expect(mockApiPost).not.toHaveBeenCalled();
  });
});

describe("startStream cleanup contract (race-condition fix)", () => {
  it("does NOT POST /api/chat/{sid}/abort during cleanup of the PREVIOUS stream", () => {
    // Setup: an existing stream with a sessionId — same sid the NEW stream
    // is about to use. Previously this POST would fire and cancel the new
    // stream on the backend (same session cancel token).
    const store = makeStore({
      agents: {
        Arty: {
          ...emptyAgentState(),
          activeSessionId: "session-shared",
          connectionPhase: "streaming",
        },
      },
    });
    const renderer = createStreamingRenderer(store);

    // Start a first stream so there's a controller in flight.
    renderer.startStream("Arty", "session-shared", [], "first prompt");
    mockApiPost.mockClear();

    // Start a SECOND stream on the SAME session id. The cleanup of the
    // previous stream's controller must NOT POST /abort (would cancel the
    // backend registration that the second stream is about to reuse).
    renderer.startStream("Arty", "session-shared", [], "second prompt");

    expect(mockApiPost).not.toHaveBeenCalledWith(
      "/api/chat/session-shared/abort",
    );
    // And strictly: no /abort POST at all during startStream cleanup.
    const abortCalls = mockApiPost.mock.calls.filter(([url]) =>
      typeof url === "string" && url.endsWith("/abort"),
    );
    expect(abortCalls).toHaveLength(0);
  });
});

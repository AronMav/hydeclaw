// ── MEM-01: Map cleanup verification ────────────────────────────────────────
// Proves that streaming-renderer's internal Maps are cleaned up on agent switch/deletion.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createStreamingRenderer } from "../streaming-renderer";

// ── Mocks ──────────────────────────────────────────────────────────────────

// Mock all imports that streaming-renderer uses
vi.mock("@/stores/sse-events", () => ({
  parseSSELines: vi.fn(() => []),
  parseSseEvent: vi.fn(() => null),
  parseContentParts: vi.fn(() => []),
}));

vi.mock("@/lib/message-parser", () => ({
  IncrementalParser: vi.fn().mockImplementation(() => ({
    processDelta: vi.fn(),
    snapshot: vi.fn(() => []),
    flush: vi.fn(() => []),
    reset: vi.fn(),
  })),
}));

vi.mock("@/lib/api", () => ({
  apiPatch: vi.fn(() => Promise.resolve()),
  getToken: vi.fn(() => "test-token"),
  assertToken: vi.fn(() => "test-token"),
}));

vi.mock("@/lib/query-client", () => ({
  queryClient: {
    getQueryData: vi.fn(() => null),
    invalidateQueries: vi.fn(),
  },
}));

vi.mock("@/lib/queries", () => ({
  qk: {
    sessions: (agent: string) => ["sessions", agent],
    sessionMessages: (id: string) => ["sessionMessages", id],
  },
}));

vi.mock("../chat-history", () => ({
  getCachedHistoryMessages: vi.fn(() => []),
}));

// ── Test helpers ───────────────────────────────────────────────────────────

function createMockStoreAccess() {
  const state: Record<string, unknown> = { agents: {}, sessionParticipants: {} };
  return {
    get: () => ({
      ...state,
      updateSessionParticipants: vi.fn(),
    }),
    set: vi.fn((fn: (draft: Record<string, unknown>) => void) => {
      fn(state);
    }),
  };
}

// ── Tests ──────────────────────────────────────────────────────────────────

describe("MEM-01: streaming-renderer Map cleanup", () => {
  let originalFetch: typeof globalThis.fetch;
  let originalRaf: typeof globalThis.requestAnimationFrame;

  beforeEach(() => {
    originalFetch = globalThis.fetch;
    originalRaf = globalThis.requestAnimationFrame;
    globalThis.fetch = vi.fn(() =>
      Promise.resolve(new Response(null, { status: 204 }))
    ) as unknown as typeof fetch;
    globalThis.requestAnimationFrame = vi.fn((cb) => {
      cb(0);
      return 0;
    }) as unknown as typeof requestAnimationFrame;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    globalThis.requestAnimationFrame = originalRaf;
  });

  it("createStreamingRenderer returns an object with cleanupAgent method", () => {
    const store = createMockStoreAccess();
    const renderer = createStreamingRenderer(store);
    expect(renderer.cleanupAgent).toBeDefined();
    expect(typeof renderer.cleanupAgent).toBe("function");
  });

  it("after startStream for agent 'A', isAgentStreaming('A') returns true", () => {
    const store = createMockStoreAccess();
    const renderer = createStreamingRenderer(store);
    renderer.startStream("A", null, [], "hello");
    expect(renderer.isAgentStreaming("A")).toBe(true);
  });

  it("after cleanupAgent('A'), isAgentStreaming('A') returns false", () => {
    const store = createMockStoreAccess();
    const renderer = createStreamingRenderer(store);
    renderer.startStream("A", null, [], "hello");
    expect(renderer.isAgentStreaming("A")).toBe(true);
    renderer.cleanupAgent("A");
    expect(renderer.isAgentStreaming("A")).toBe(false);
  });

  it("after 10 sequential agent switches, no stale entries remain", () => {
    const store = createMockStoreAccess();
    const renderer = createStreamingRenderer(store);
    const agents = Array.from({ length: 10 }, (_, i) => `agent-${i}`);

    for (const agent of agents) {
      renderer.startStream(agent, null, [], "hello");
      expect(renderer.isAgentStreaming(agent)).toBe(true);
      renderer.cleanupAgent(agent);
    }

    // Verify all agents are cleaned up — no stale entries
    for (const agent of agents) {
      expect(renderer.isAgentStreaming(agent)).toBe(false);
    }
  });

  it("cleanupAgent for non-existent agent does not throw", () => {
    const store = createMockStoreAccess();
    const renderer = createStreamingRenderer(store);
    expect(() => renderer.cleanupAgent("nonexistent")).not.toThrow();
    expect(() => renderer.cleanupAgent("nonexistent")).not.toThrow(); // double cleanup is idempotent
  });
});

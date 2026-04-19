import { describe, it, expect, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { useEngineRunning } from "../use-engine-running";

// Mock useSessions — return controlled run_status.
vi.mock("@/lib/queries", () => ({
  useSessions: vi.fn(() => ({ data: { sessions: [] } })),
}));

// Mock useChatStore — read-only subscription helper in tests.
vi.mock("@/stores/chat-store", () => {
  const state = {
    agents: {
      Arty: {
        activeSessionId: "s1",
        connectionPhase: "idle",
        activeSessionIds: [],
      },
    },
  };
  return {
    useChatStore: (selector: any) => selector(state),
  };
});

describe("useEngineRunning", () => {
  it("returns false when all three signals say idle", () => {
    const { result } = renderHook(() => useEngineRunning("Arty"));
    expect(result.current).toBe(false);
  });
});

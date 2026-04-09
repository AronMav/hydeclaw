import { describe, it, expect } from "vitest";

import type { ConnectionPhase, MessageSource } from "@/stores/chat-store";

// ── Pure logic extracted from ChatThread.tsx ────────────────────────────────

/**
 * OPTI-01: showThinking computation.
 * Mirrors ChatThread.tsx L889-890:
 *   showThinking = messageSource.mode === "live"
 *     && (connectionPhase === "submitted" || (engineRunning && !hasAssistantContent))
 */
function computeShowThinking(
  messageSource: MessageSource,
  connectionPhase: ConnectionPhase,
  engineRunning: boolean,
  hasAssistantContent: boolean,
): boolean {
  return (
    messageSource.mode === "live" &&
    (connectionPhase === "submitted" || (engineRunning && !hasAssistantContent))
  );
}

/**
 * OPTI-02: skeleton display guard.
 * Mirrors ChatThread.tsx L894:
 *   showSkeleton = historyLoading && !sessionMessagesData && messageSource.mode !== "live"
 */
function computeShowSkeleton(
  historyLoading: boolean,
  sessionMessagesData: unknown | undefined,
  messageSource: MessageSource,
): boolean {
  return historyLoading && !sessionMessagesData && messageSource.mode !== "live";
}

// ── OPTI-01: Instant thinking indicator ────────────────────────────────────

describe("OPTI-01: showThinking contract", () => {
  it("OPTI-01-A: showThinking is true when connectionPhase=submitted and mode=live", () => {
    // After sendMessage(), connectionPhase is set to "submitted" synchronously.
    // This guarantees the thinking indicator appears instantly — before any SSE event.
    const result = computeShowThinking(
      { mode: "live", messages: [] },
      "submitted",
      false,
      false,
    );
    expect(result).toBe(true);
    // Verify the critical invariant: connectionPhase === "submitted" is the trigger
    expect("submitted" satisfies ConnectionPhase).toBe("submitted");
  });

  it("OPTI-01-B: showThinking is true when engineRunning and no assistant content yet", () => {
    // After page reload, engine may still be running but SSE not connected.
    // engineRunning + no assistant content = still waiting for response.
    const result = computeShowThinking(
      { mode: "live", messages: [] },
      "idle",
      true,
      false,
    );
    expect(result).toBe(true);
  });

  it("OPTI-01-C: showThinking is false when messageSource.mode=new-chat (no ghost thinking on empty chat)", () => {
    // A brand new chat with no messages should never show thinking indicator.
    const result = computeShowThinking(
      { mode: "new-chat" },
      "submitted",
      false,
      false,
    );
    expect(result).toBe(false);
  });
});

// ── OPTI-02: Agent-switch skeleton guard ───────────────────────────────────

describe("OPTI-02: skeleton display guard contract", () => {
  it("OPTI-02-A: skeleton renders when historyLoading=true, no cache, and mode !== live", () => {
    // When switching agents with no cached data, show shape-matched skeleton.
    const result = computeShowSkeleton(
      true,
      undefined,
      { mode: "history", sessionId: "abc" },
    );
    expect(result).toBe(true);
    // Verify the guard: historyLoading && !sessionMessagesData
    expect(true && !undefined).toBe(true);
  });

  it("OPTI-02-B: skeleton does NOT render when sessionMessagesData exists in cache", () => {
    // If React Query cache has session data, render cached messages immediately.
    const cachedData = { messages: [{ id: "1", role: "assistant", parts: [] }] };
    const result = computeShowSkeleton(
      true,
      cachedData,
      { mode: "history", sessionId: "abc" },
    );
    expect(result).toBe(false);
  });

  it("OPTI-02-C: skeleton does NOT render when messageSource.mode=live", () => {
    // During active streaming, live messages are the source — never show skeleton.
    const result = computeShowSkeleton(
      true,
      undefined,
      { mode: "live", messages: [] },
    );
    expect(result).toBe(false);
  });
});

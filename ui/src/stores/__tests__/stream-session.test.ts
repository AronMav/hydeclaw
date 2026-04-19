// ui/src/stores/__tests__/stream-session.test.ts
import { describe, it, expect, vi, beforeEach } from "vitest";
import { StreamSession, streamSessionManager } from "../stream-session";
import { useChatStore } from "../chat-store";

beforeEach(() => {
  // Reset the store to a known state with one agent.
  useChatStore.setState((draft: any) => {
    draft.agents = {
      Arty: {
        activeSessionId: null,
        activeSessionIds: [],
        messageSource: { mode: "new-chat" },
        connectionPhase: "idle",
        connectionError: null,
        streamError: null,
        streamGeneration: 0,
        reconnectAttempt: 0,
        selectedBranches: {},
        renderLimit: 100,
        turnLimitMessage: null,
        maxReconnectAttempts: 3,
      },
    };
  });
  streamSessionManager.disposeCurrent("Arty");
});

describe("StreamSession", () => {
  it("write applies when session is current", () => {
    const s = streamSessionManager.start("Arty");
    s.write({ connectionPhase: "streaming" });
    expect(useChatStore.getState().agents.Arty.connectionPhase).toBe("streaming");
  });

  it("write is a no-op after dispose", () => {
    const s = streamSessionManager.start("Arty");
    s.dispose();
    s.write({ connectionPhase: "streaming" });
    // dispose() itself writes connectionPhase: "idle" as the final legal write.
    expect(useChatStore.getState().agents.Arty.connectionPhase).toBe("idle");
  });

  it("write is a no-op when a new session superseded us", () => {
    const s1 = streamSessionManager.start("Arty");
    streamSessionManager.start("Arty"); // bumps generation, disposes s1
    s1.write({ connectionPhase: "streaming" });
    // s2 is fresh; connectionPhase should be "idle" (the dispose-write landed for s1, no writes from s2 yet)
    expect(useChatStore.getState().agents.Arty.connectionPhase).toBe("idle");
  });

  it("writeDraft hands back the agent's draft, not root state", () => {
    const s = streamSessionManager.start("Arty");
    s.writeDraft((agent) => { agent.streamError = "test"; });
    expect(useChatStore.getState().agents.Arty.streamError).toBe("test");
  });

  it("dispose is idempotent", () => {
    const s = streamSessionManager.start("Arty");
    s.dispose();
    s.dispose(); // must not throw
    expect(s.disposed).toBe(true);
  });

  it("dispose aborts the signal", () => {
    const s = streamSessionManager.start("Arty");
    expect(s.signal.aborted).toBe(false);
    s.dispose();
    expect(s.signal.aborted).toBe(true);
  });

  it("streamSessionManager.start disposes previous session for same agent", () => {
    const s1 = streamSessionManager.start("Arty");
    const s2 = streamSessionManager.start("Arty");
    expect(s1.disposed).toBe(true);
    expect(s2.disposed).toBe(false);
  });

  it("start bumps generation exactly once per logical transition", () => {
    const g0 = useChatStore.getState().agents.Arty.streamGeneration;
    streamSessionManager.start("Arty");
    const g1 = useChatStore.getState().agents.Arty.streamGeneration;
    expect(g1).toBe(g0 + 1);
  });

  it("disposeCurrent bumps generation exactly once (no-op if no active)", () => {
    const g0 = useChatStore.getState().agents.Arty.streamGeneration;
    streamSessionManager.start("Arty");
    const g1 = useChatStore.getState().agents.Arty.streamGeneration;
    streamSessionManager.disposeCurrent("Arty");
    const g2 = useChatStore.getState().agents.Arty.streamGeneration;
    expect(g1).toBe(g0 + 1);
    expect(g2).toBe(g1 + 1);
    streamSessionManager.disposeCurrent("Arty"); // no-op when no active
    const g3 = useChatStore.getState().agents.Arty.streamGeneration;
    expect(g3).toBe(g2);
  });

  it("dev-mode debug log fires on dropped write", () => {
    const spy = vi.spyOn(console, "debug").mockImplementation(() => {});
    const s = streamSessionManager.start("Arty");
    s.dispose();
    s.write({ connectionPhase: "streaming" });
    expect(spy).toHaveBeenCalledOnce();
    spy.mockRestore();
  });
});

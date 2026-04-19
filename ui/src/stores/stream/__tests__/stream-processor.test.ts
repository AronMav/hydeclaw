import { describe, it, expect, vi, beforeEach } from "vitest";
import { processSSEStream } from "../stream-processor";
import { streamSessionManager } from "../../stream-session";
import { useChatStore } from "../../chat-store";

// Build a ReadableStream from an array of frames (SSE "data: ...\n\n" format).
function makeStream(frames: string[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  let i = 0;
  return new ReadableStream({
    pull(controller) {
      if (i < frames.length) {
        controller.enqueue(encoder.encode(frames[i++]));
      } else {
        controller.close();
      }
    },
  });
}

// Minimal callbacks for processSSEStream.
function makeCallbacks(overrides: Partial<Parameters<typeof processSSEStream>[2]["callbacks"]> = {}) {
  return {
    onSessionId: vi.fn(),
    onReconnectNeeded: vi.fn(),
    getAgentState: (agent: string) => useChatStore.getState().agents[agent],
    updateSessionParticipants: vi.fn(),
    onStreamDone: vi.fn(),
    ...overrides,
  };
}

beforeEach(() => {
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
        modelOverride: null,
        forceNewSession: false,
      },
    };
  });
  streamSessionManager.disposeCurrent("Arty");
});

describe("processSSEStream", () => {
  it("invokes onSessionId on first data-session-id frame", async () => {
    const session = streamSessionManager.start("Arty");
    const callbacks = makeCallbacks();
    const frames = [
      `data: ${JSON.stringify({ type: "data-session-id", data: { sessionId: "s1" } })}\n\n`,
    ];
    await processSSEStream(session, makeStream(frames), {
      sessionId: null,
      reconnectAttempt: 0,
      callbacks,
    });
    expect(callbacks.onSessionId).toHaveBeenCalledWith("s1");
  });

  it("signals reconnect-needed when stream ends without finish event", async () => {
    const session = streamSessionManager.start("Arty");
    const callbacks = makeCallbacks();
    const frames = [
      `data: ${JSON.stringify({ type: "data-session-id", data: { sessionId: "s1" } })}\n\n`,
      `data: ${JSON.stringify({ type: "text-delta", delta: "hi", id: "t1" })}\n\n`,
      // no finish event — stream closes
    ];
    await processSSEStream(session, makeStream(frames), {
      sessionId: null,
      reconnectAttempt: 0,
      callbacks,
    });
    expect(callbacks.onReconnectNeeded).toHaveBeenCalled();
  });

  it("does not call onReconnectNeeded when stream ends with finish event", async () => {
    const session = streamSessionManager.start("Arty");
    const callbacks = makeCallbacks();
    const frames = [
      `data: ${JSON.stringify({ type: "data-session-id", data: { sessionId: "s1" } })}\n\n`,
      `data: ${JSON.stringify({ type: "text-start", id: "t1" })}\n\n`,
      `data: ${JSON.stringify({ type: "text-delta", delta: "hi", id: "t1" })}\n\n`,
      `data: ${JSON.stringify({ type: "text-end", id: "t1" })}\n\n`,
      `data: ${JSON.stringify({ type: "finish" })}\n\n`,
    ];
    await processSSEStream(session, makeStream(frames), {
      sessionId: null,
      reconnectAttempt: 0,
      callbacks,
    });
    expect(callbacks.onReconnectNeeded).not.toHaveBeenCalled();
  });
});

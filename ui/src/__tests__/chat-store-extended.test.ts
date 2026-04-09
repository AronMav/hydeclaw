import { vi, describe, it, expect } from "vitest";

// Mock dependencies before importing chat-store
vi.mock("@/lib/query-client", () => ({
  queryClient: { invalidateQueries: vi.fn(), getQueryData: vi.fn(() => undefined) },
}));
vi.mock("@/lib/api", () => ({
  apiGet: vi.fn(),
  apiDelete: vi.fn(),
  apiPatch: vi.fn(),
  getToken: vi.fn(() => "test-token"),
}));

import {
  isActiveStream,
  convertHistory,
  MAX_INPUT_LENGTH,
  getInitialAgent,
  getLastSessionId,
  saveLastSession,
} from "@/stores/chat-store";
import type { MessageRow } from "@/types/api";

// ── Constants ───────────────────────────────────────────────────────────────

describe("MAX_INPUT_LENGTH", () => {
  it("is 32000", () => {
    expect(MAX_INPUT_LENGTH).toBe(32_000);
  });
});

// ── convertHistory ──────────────────────────────────────────────────────────

function makeRow(overrides: Partial<MessageRow>): MessageRow {
  return {
    id: "m1",
    role: "user",
    content: "",
    tool_calls: null,
    tool_call_id: null,
    created_at: "2026-01-01T00:00:00Z",
    status: "done",
    feedback: 0,
    edited_at: null,
    ...overrides,
  };
}

describe("convertHistory", () => {
  it("converts a simple user+assistant exchange", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "Hello" }),
      makeRow({ id: "a1", role: "assistant", content: "Hi there" }),
    ];
    const msgs = convertHistory(rows);
    expect(msgs).toHaveLength(2);
    expect(msgs[0].role).toBe("user");
    expect(msgs[0].parts).toEqual([{ type: "text", text: "Hello" }]);
    expect(msgs[1].role).toBe("assistant");
    expect(msgs[1].parts[0]).toEqual({ type: "text", text: "Hi there" });
  });

  it("filters out streaming messages", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "Hi" }),
      makeRow({ id: "a1", role: "assistant", content: "partial...", status: "streaming" }),
      makeRow({ id: "a2", role: "assistant", content: "Full response", status: "complete" }),
    ];
    const msgs = convertHistory(rows);
    const assistantMsgs = msgs.filter(m => m.role === "assistant");
    expect(assistantMsgs).toHaveLength(1);
    expect(assistantMsgs[0].parts[0]).toEqual({ type: "text", text: "Full response" });
  });

  it("extracts <think> blocks as reasoning parts", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "question" }),
      makeRow({
        id: "a1",
        role: "assistant",
        content: "<think>Let me think...</think>The answer is 42.",
      }),
    ];
    const msgs = convertHistory(rows);
    const parts = msgs[1].parts;
    expect(parts).toHaveLength(2);
    expect(parts[0]).toEqual({ type: "reasoning", text: "Let me think..." });
    expect(parts[1]).toEqual({ type: "text", text: "The answer is 42." });
  });

  it("handles tool call lifecycle (assistant+tool rows)", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "search for cats" }),
      makeRow({
        id: "a1",
        role: "assistant",
        content: "",
        tool_calls: [{ id: "tc1", name: "search", arguments: { q: "cats" } }],
      }),
      makeRow({
        id: "t1",
        role: "tool",
        content: "Found 5 results",
        tool_call_id: "tc1",
      }),
      makeRow({ id: "a2", role: "assistant", content: "Here are your results." }),
    ];
    const msgs = convertHistory(rows);
    // Should have: user, assistant (with tool part), assistant (text)
    expect(msgs.length).toBeGreaterThanOrEqual(2);
    const toolPart = msgs.flatMap(m => m.parts).find(p => p.type === "tool");
    expect(toolPart?.type).toBe("tool");
    if (toolPart?.type === "tool") {
      expect(toolPart.toolName).toBe("search");
      expect(toolPart.state).toBe("output-available");
      expect(toolPart.output).toBe("Found 5 results");
    }
  });

  it("extracts __file__ markers from tool output", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "show image" }),
      makeRow({
        id: "a1",
        role: "assistant",
        content: "",
        tool_calls: [{ id: "tc1", name: "img", arguments: {} }],
      }),
      makeRow({
        id: "t1",
        role: "tool",
        content: '__file__:{"url":"/img.png","mediaType":"image/png"}\nDone',
        tool_call_id: "tc1",
      }),
    ];
    const msgs = convertHistory(rows);
    const parts = msgs.flatMap(m => m.parts);
    const filePart = parts.find(p => p.type === "file");
    expect(filePart).toEqual({ type: "file", url: "/img.png", mediaType: "image/png" });
    const toolPart = parts.find(p => p.type === "tool");
    if (toolPart?.type === "tool") {
      expect(toolPart.output).toBe("Done");
    }
  });

  it("returns empty array for empty input", () => {
    expect(convertHistory([])).toEqual([]);
  });

  it("preserves agentId from rows", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "hi", agent_id: "Agent1" }),
    ];
    const msgs = convertHistory(rows);
    expect(msgs[0].agentId).toBe("Agent1");
  });
});

// ── localStorage helpers ────────────────────────────────────────────────────

describe("getInitialAgent", () => {
  it("returns first agent when nothing saved", () => {
    localStorage.removeItem("hydeclaw.lastSession");
    expect(getInitialAgent(["A", "B"])).toBe("A");
  });

  it("returns empty string for empty list", () => {
    expect(getInitialAgent([])).toBe("");
  });
});

describe("saveLastSession / getLastSessionId", () => {
  it("saves and retrieves session id per agent", () => {
    saveLastSession("Agent1", "sess-1");
    expect(getLastSessionId("Agent1")).toBe("sess-1");
  });

  it("returns undefined for unknown agent", () => {
    localStorage.removeItem("hydeclaw.lastSession");
    expect(getLastSessionId("Unknown")).toBeUndefined();
  });
});

// ── STATE-01: history to live transition ────────────────────────────────────

describe("STATE-01: history to live transition", () => {
  it("sendMessage from history does not set viewMode live before startStream populates liveMessages", async () => {
    const { useChatStore } = await import("@/stores/chat-store");

    // Set up agent in history mode with an active session
    useChatStore.setState((s) => {
      s.currentAgent = "TestAgent";
      if (!s.agents["TestAgent"]) {
        s.agents["TestAgent"] = {
          activeSessionId: "sess-history",
          liveMessages: [],
          viewMode: "history",
          streamStatus: "idle",
          streamError: null,
          forceNewSession: false,
          thinkingSessionId: null,
          activeSessionIds: [],
          renderLimit: 100,
          modelOverride: null,
          pendingTargetAgent: null,
          agentTurns: [],
          turnCount: 0,
          turnLimitMessage: null,
        };
      } else {
        s.agents["TestAgent"].viewMode = "history";
        s.agents["TestAgent"].activeSessionId = "sess-history";
        s.agents["TestAgent"].liveMessages = [];
        s.agents["TestAgent"].streamStatus = "idle";
      }
    });

    // Record all states during sendMessage
    const stateSnapshots: Array<{ viewMode: string; liveMessages: unknown[] }> = [];
    const unsub = useChatStore.subscribe((state) => {
      const ag = state.agents["TestAgent"];
      if (ag) {
        stateSnapshots.push({ viewMode: ag.viewMode, liveMessages: [...ag.liveMessages] });
      }
    });

    // Mock fetch to prevent actual network calls
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(new ReadableStream(), { status: 200 })
    );

    useChatStore.getState().sendMessage("hello");

    unsub();
    fetchSpy.mockRestore();

    // After sendMessage, every state transition to "live" must have non-empty liveMessages.
    // The early flip (viewMode:"live" before startStream) would produce an empty-liveMessages snapshot.
    const liveTransitions = stateSnapshots.filter((s) => s.viewMode === "live");
    for (const snap of liveTransitions) {
      expect(snap.liveMessages.length).toBeGreaterThan(0);
    }
  });

  it("chat-store.ts sendMessage has no early viewMode flip before startStream", async () => {
    // Static analysis: the three early `update(agent, { viewMode: "live" })` calls
    // in sendMessage/regenerate/regenerateFrom must be removed.
    const fs = await import("node:fs");
    const src = fs.readFileSync(
      new URL("../../stores/chat-store.ts", import.meta.url).pathname,
      "utf8"
    );

    // sendMessage block — from "sendMessage: " to "stopStream:"
    const sendMessageBlock = src.slice(
      src.indexOf("sendMessage: (text: string)"),
      src.indexOf("stopStream:")
    );
    expect(sendMessageBlock).not.toMatch(/update\(agent,\s*\{\s*viewMode:\s*["']live["']\s*\}/);

    // regenerate block — from "regenerate: () =>" to "regenerateFrom:"
    const regenerateBlock = src.slice(
      src.indexOf("regenerate: ()"),
      src.indexOf("regenerateFrom:")
    );
    expect(regenerateBlock).not.toMatch(/update\(agent,\s*\{\s*viewMode:\s*["']live["']\s*\}/);

    // regenerateFrom block — find the block after regenerate
    const regenerateFromStart = src.indexOf("regenerateFrom: (messageId");
    const regenerateFromEnd = src.indexOf("stopStream:", regenerateFromStart);
    const regenerateFromBlock = src.slice(regenerateFromStart, regenerateFromEnd);
    expect(regenerateFromBlock).not.toMatch(/update\(agent,\s*\{\s*viewMode:\s*["']live["']\s*\}/);
  });
});

// ── STATE-02: beforeunload flush ─────────────────────────────────────────────

describe("STATE-02: beforeunload flush", () => {
  it("chat-store.ts registers beforeunload on window", async () => {
    const fs = await import("node:fs");
    const src = fs.readFileSync(
      new URL("../../stores/chat-store.ts", import.meta.url).pathname,
      "utf8"
    );
    expect(src).toContain("beforeunload");
    expect(src).toContain("keepalive: true");
  });

  it("beforeunload fires keepalive fetch with correct session payload", async () => {
    // Capture the beforeunload handler registered when module loads
    let capturedHandler: (() => void) | null = null;
    const origAdd = window.addEventListener.bind(window);
    const addEventSpy = vi
      .spyOn(window, "addEventListener")
      .mockImplementation((type: string, handler: EventListenerOrEventListenerObject, ...rest) => {
        if (type === "beforeunload") {
          capturedHandler = handler as () => void;
        }
        return origAdd(type, handler, ...(rest as []));
      });

    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response());

    vi.resetModules();
    vi.mock("@/lib/query-client", () => ({
      queryClient: { invalidateQueries: vi.fn(), getQueryData: vi.fn(() => undefined) },
    }));
    vi.mock("@/lib/api", () => ({
      apiGet: vi.fn(),
      apiDelete: vi.fn(),
      apiPatch: vi.fn(),
      getToken: vi.fn(() => "bearer-xyz"),
    }));

    const { useChatStore } = await import("@/stores/chat-store");

    // Set up agent state with active session
    useChatStore.setState((s) => {
      s.currentAgent = "Alpha";
      if (!s.agents["Alpha"]) {
        s.agents["Alpha"] = {
          activeSessionId: "sess-abc",
          liveMessages: [],
          viewMode: "live",
          streamStatus: "idle",
          streamError: null,
          forceNewSession: false,
          thinkingSessionId: null,
          activeSessionIds: [],
          renderLimit: 100,
          modelOverride: null,
          pendingTargetAgent: null,
          agentTurns: [],
          turnCount: 0,
          turnLimitMessage: null,
        };
      } else {
        s.agents["Alpha"].activeSessionId = "sess-abc";
        s.agents["Alpha"].viewMode = "live";
        s.agents["Alpha"].streamStatus = "idle";
      }
    });

    expect(capturedHandler).not.toBeNull();
    capturedHandler!();

    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/sessions/sess-abc",
      expect.objectContaining({
        method: "PATCH",
        keepalive: true,
      })
    );

    const callArgs = fetchSpy.mock.calls[0][1] as RequestInit;
    expect(callArgs.keepalive).toBe(true);
    const body = JSON.parse(callArgs.body as string);
    expect(body.ui_state).toBeDefined();
    expect(body.ui_state.viewMode).toBe("live");

    addEventSpy.mockRestore();
    fetchSpy.mockRestore();
  });

  it("beforeunload cancels pending saveUiState timer", async () => {
    const clearSpy = vi.spyOn(globalThis, "clearTimeout");
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response());

    let capturedHandler: (() => void) | null = null;
    const origAdd = window.addEventListener.bind(window);
    const addEventSpy = vi
      .spyOn(window, "addEventListener")
      .mockImplementation((type: string, handler: EventListenerOrEventListenerObject, ...rest) => {
        if (type === "beforeunload") {
          capturedHandler = handler as () => void;
        }
        return origAdd(type, handler, ...(rest as []));
      });

    vi.resetModules();
    vi.mock("@/lib/query-client", () => ({
      queryClient: { invalidateQueries: vi.fn(), getQueryData: vi.fn(() => undefined) },
    }));
    vi.mock("@/lib/api", () => ({
      apiGet: vi.fn(),
      apiDelete: vi.fn(),
      apiPatch: vi.fn(),
      getToken: vi.fn(() => "tok"),
    }));

    const { useChatStore } = await import("@/stores/chat-store");
    useChatStore.setState((s) => {
      s.currentAgent = "Beta";
      if (!s.agents["Beta"]) {
        s.agents["Beta"] = {
          activeSessionId: "sess-beta",
          liveMessages: [],
          viewMode: "history",
          streamStatus: "idle",
          streamError: null,
          forceNewSession: false,
          thinkingSessionId: null,
          activeSessionIds: [],
          renderLimit: 100,
          modelOverride: null,
          pendingTargetAgent: null,
          agentTurns: [],
          turnCount: 0,
          turnLimitMessage: null,
        };
      } else {
        s.agents["Beta"].activeSessionId = "sess-beta";
        s.agents["Beta"].viewMode = "history";
        s.agents["Beta"].streamStatus = "idle";
      }
    });

    expect(capturedHandler).not.toBeNull();
    capturedHandler!();

    // clearTimeout should have been called (to cancel any pending debounced save)
    expect(clearSpy).toHaveBeenCalled();

    addEventSpy.mockRestore();
    fetchSpy.mockRestore();
    clearSpy.mockRestore();
  });
});

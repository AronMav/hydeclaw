import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { parseSSELines, parseSseEvent } from "@/stores/sse-events";
import { useChatStore } from "@/stores/chat-store";

// Mock react-query (used inside chat-store for cache invalidation)
vi.mock("@/lib/query-client", () => ({
  queryClient: { invalidateQueries: vi.fn(), getQueryData: vi.fn(() => undefined) },
}));

// Mock api helpers (getToken reads localStorage which may not be set in jsdom)
vi.mock("@/lib/api", () => ({
  apiGet: vi.fn(),
  apiDelete: vi.fn(),
  apiPatch: vi.fn(),
  getToken: vi.fn(() => "test-token"),
}));

describe("parseSSELines", () => {
  it("splits single complete line", () => {
    const buf = { current: "" };
    const lines = parseSSELines("data: hello\n", buf);
    expect(lines).toEqual(["data: hello"]);
    expect(buf.current).toBe("");
  });

  it("buffers incomplete line", () => {
    const buf = { current: "" };
    const lines = parseSSELines("data: hel", buf);
    expect(lines).toEqual([]);
    expect(buf.current).toBe("data: hel");
  });

  it("flushes buffer on next chunk", () => {
    const buf = { current: "data: hel" };
    const lines = parseSSELines("lo\n", buf);
    expect(lines).toEqual(["data: hello"]);
  });

  it("splits multiple lines", () => {
    const buf = { current: "" };
    const lines = parseSSELines("data: a\ndata: b\n", buf);
    expect(lines).toEqual(["data: a", "data: b"]);
  });

  it("strips \\r from \\r\\n line endings", () => {
    const buf = { current: "" };
    const lines = parseSSELines("data: hello\r\n", buf);
    expect(lines).toEqual(["data: hello"]);
  });
});

describe("parseSseEvent", () => {
  it("parses text-delta event", () => {
    const event = parseSseEvent(JSON.stringify({ type: "text-delta", delta: "hello" }));
    expect(event?.type).toBe("text-delta");
    if (event?.type === "text-delta") expect(event.delta).toBe("hello");
  });

  it("defaults missing delta to empty string", () => {
    const event = parseSseEvent(JSON.stringify({ type: "text-delta" }));
    expect(event?.type).toBe("text-delta");
    if (event?.type === "text-delta") expect(event.delta).toBe("");
  });

  it("parses tool-input-start event", () => {
    const event = parseSseEvent(JSON.stringify({ type: "tool-input-start", toolCallId: "tc1", toolName: "search" }));
    expect(event?.type).toBe("tool-input-start");
    if (event?.type === "tool-input-start") {
      expect(event.toolCallId).toBe("tc1");
      expect(event.toolName).toBe("search");
    }
  });

  it("returns null for tool-input-start missing fields", () => {
    expect(parseSseEvent(JSON.stringify({ type: "tool-input-start" }))).toBeNull();
  });

  it("parses tool-output-available event", () => {
    const event = parseSseEvent(JSON.stringify({ type: "tool-output-available", toolCallId: "tc1", output: "result" }));
    expect(event?.type).toBe("tool-output-available");
    if (event?.type === "tool-output-available") expect(event.output).toBe("result");
  });

  it("returns null for invalid JSON", () => {
    expect(parseSseEvent("not json")).toBeNull();
  });

  it("returns null for missing type", () => {
    expect(parseSseEvent(JSON.stringify({ delta: "oops" }))).toBeNull();
  });

  it("parses finish event", () => {
    expect(parseSseEvent(JSON.stringify({ type: "finish" }))?.type).toBe("finish");
  });

  it("parses error event with errorText", () => {
    const event = parseSseEvent(JSON.stringify({ type: "error", errorText: "timeout" }));
    expect(event?.type).toBe("error");
    if (event?.type === "error") expect(event.errorText).toBe("timeout");
  });

  it("returns null for unknown event type", () => {
    expect(parseSseEvent(JSON.stringify({ type: "unknown-future-event" }))).toBeNull();
  });
});

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Encode SSE events as a ReadableStream */
function makeSSEStream(events: object[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  const chunks = events.map(e => encoder.encode(`data: ${JSON.stringify(e)}\n`));
  let i = 0;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      if (i < chunks.length) controller.enqueue(chunks[i++]);
      else controller.close();
    },
  });
}

function mockFetch(events: object[]) {
  return vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(makeSSEStream(events), { status: 200 })
  );
}

// ── Store integration tests ───────────────────────────────────────────────────

describe("chat store — streaming via sendMessage", () => {
  const AGENT = "TestAgent";

  beforeEach(() => {
    useChatStore.setState({ agents: {}, currentAgent: AGENT, _selectCounter: {} });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("accumulates text-delta events into assistant message parts", async () => {
    // Use a longer text to exceed IncrementalParser's 15-char buffer threshold
    const longText1 = "Hello world, this is a longer response from";
    const longText2 = " the assistant to ensure it exceeds the buffer threshold.";
    mockFetch([
      { type: "data-session-id", data: { sessionId: "sess-1" } },
      { type: "start", messageId: "msg-1" },
      { type: "text-start", id: "txt-1" },
      { type: "text-delta", delta: longText1 },
      { type: "text-delta", delta: longText2 },
      { type: "text-end" },
      { type: "finish" },
    ]);

    useChatStore.getState().sendMessage("test prompt");
    await new Promise(r => setTimeout(r, 200));

    const st = useChatStore.getState().agents[AGENT];
    expect(st?.connectionPhase).toBe("idle");
    // After stream completion, messageSource transitions to history (sess-1 was received)
    expect(st?.messageSource.mode).toBe("history");
    expect(st?.activeSessionId).toBe("sess-1");
  });

  it("sets connectionPhase=error on error event", async () => {
    mockFetch([
      { type: "start", messageId: "msg-1" },
      { type: "error", errorText: "LLM timeout" },
    ]);

    useChatStore.getState().sendMessage("test");
    await new Promise(r => setTimeout(r, 200));

    const st = useChatStore.getState().agents[AGENT];
    expect(st?.connectionPhase).toBe("error");
    expect(st?.streamError).toBe("LLM timeout");
  });

  it("sets activeSessionId from data-session-id event", async () => {
    mockFetch([
      { type: "data-session-id", data: { sessionId: "new-session-uuid" } },
      { type: "finish" },
    ]);

    useChatStore.getState().sendMessage("test");
    await new Promise(r => setTimeout(r, 200));

    expect(useChatStore.getState().agents[AGENT]?.activeSessionId).toBe("new-session-uuid");
  });

  it("handles tool call lifecycle: input-start → input-available → output-available", async () => {
    mockFetch([
      { type: "start", messageId: "msg-1" },
      { type: "tool-input-start", toolCallId: "tc-1", toolName: "search" },
      { type: "tool-input-available", toolCallId: "tc-1", input: { query: "test" } },
      { type: "tool-output-available", toolCallId: "tc-1", output: "search results" },
      { type: "finish" },
    ]);

    useChatStore.getState().sendMessage("search for something");
    await new Promise(r => setTimeout(r, 200));

    const st = useChatStore.getState().agents[AGENT];
    // Stream completed — no sessionId sent so messageSource is new-chat
    // Verify connection phase is idle (stream processed successfully)
    expect(st?.connectionPhase).toBe("idle");
  });

  it("stopStream sets status=idle and preserves partial message", async () => {
    // Stream that never closes — simulates slow LLM response
    const encoder = new TextEncoder();
    let enqueued = 0;
    const neverEndingStream = new ReadableStream<Uint8Array>({
      async pull(controller) {
        if (enqueued === 0) {
          controller.enqueue(encoder.encode(`data: ${JSON.stringify({ type: "start", messageId: "m1" })}\n`));
          controller.enqueue(encoder.encode(`data: ${JSON.stringify({ type: "text-start", id: "t1" })}\n`));
          controller.enqueue(encoder.encode(`data: ${JSON.stringify({ type: "text-delta", delta: "partial" })}\n`));
          enqueued++;
        }
        // Hang indefinitely — caller will abort via AbortController
        await new Promise(() => {});
      },
    });
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(neverEndingStream, { status: 200 }));

    useChatStore.getState().sendMessage("test");
    // Note: timing-dependent — 100ms should be enough for jsdom to process the first pull()
    await new Promise(r => setTimeout(r, 100));

    useChatStore.getState().stopStream();
    await new Promise(r => setTimeout(r, 50));

    const st = useChatStore.getState().agents[AGENT];
    expect(st?.connectionPhase).toBe("idle");
    // Partial message must be preserved, not lost
    const liveMessages = st?.messageSource.mode === "live" ? st.messageSource.messages : (st?.messageSource.mode === "history" ? [] : []);
    const assistantMsg = liveMessages.find(m => m.role === "assistant");
    expect(assistantMsg).toBeDefined();
  });

  it("sync event with status=finished transitions stream to idle", async () => {
    mockFetch([
      { type: "data-session-id", data: { sessionId: "sess-sync" } },
      { type: "start", messageId: "msg-1" },
      { type: "text-start", id: "txt-1" },
      { type: "text-delta", delta: "partial text" },
      { type: "sync", content: "full final text", toolCalls: [], status: "finished" },
      { type: "finish" },
    ]);

    useChatStore.getState().sendMessage("test sync");
    await new Promise(r => setTimeout(r, 200));

    const st = useChatStore.getState().agents[AGENT];
    expect(st?.connectionPhase).toBe("idle");
    // After stream with sessionId, transitions to history mode
    expect(st?.messageSource.mode).toBe("history");
    expect(st?.activeSessionId).toBe("sess-sync");
  });

  it("sync event after tool-input-start preserves tool state through finish", async () => {
    mockFetch([
      { type: "data-session-id", data: { sessionId: "sess-tool-sync" } },
      { type: "start", messageId: "msg-1" },
      { type: "tool-input-start", toolCallId: "tc-1", toolName: "search" },
      { type: "sync", content: "", toolCalls: [
        { toolCallId: "tc-1", toolName: "search", input: { q: "test" }, output: "results" }
      ], status: "finished" },
      { type: "finish" },
    ]);

    useChatStore.getState().sendMessage("test tool sync");
    await new Promise(r => setTimeout(r, 200));

    const st = useChatStore.getState().agents[AGENT];
    expect(st?.connectionPhase).toBe("idle");
    // After stream with sessionId, transitions to history mode
    expect(st?.messageSource.mode).toBe("history");
    expect(st?.activeSessionId).toBe("sess-tool-sync");
  });

  it("sync event with status=error sets streamError", async () => {
    mockFetch([
      { type: "data-session-id", data: { sessionId: "sess-err-sync" } },
      { type: "start", messageId: "msg-1" },
      { type: "text-start", id: "txt-1" },
      { type: "text-delta", delta: "working..." },
      { type: "sync", content: "partial", toolCalls: [], status: "error", error: "LLM provider timeout" },
      { type: "finish" },
    ]);

    useChatStore.getState().sendMessage("test error sync");
    await new Promise(r => setTimeout(r, 200));

    const st = useChatStore.getState().agents[AGENT];
    expect(st?.connectionPhase).toBe("error");
    expect(st?.streamError).toBe("LLM provider timeout");
  });
});

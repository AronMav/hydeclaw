import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { SseConnection } from "@/lib/sse-connection";
import type { SseConnectionConfig, SseConnectionCallbacks } from "@/lib/sse-connection";

// ── Helpers ──────────────────────────────────────────────────────────────────

const encoder = new TextEncoder();

/**
 * Creates a ReadableStream that emits SSE-formatted chunks, one per event.
 * Each chunk is a complete "data: <json>\n" line.
 */
function createMockStream(chunks: string[]): ReadableStream<Uint8Array> {
  let i = 0;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      if (i < chunks.length) {
        controller.enqueue(encoder.encode(chunks[i++]));
      } else {
        controller.close();
      }
    },
  });
}

function sseChunk(event: object): string {
  return `data: ${JSON.stringify(event)}\n`;
}

function mockFetchOk(chunks: string[]): void {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(createMockStream(chunks), { status: 200 }),
  );
}

function makeCallbacks(): SseConnectionCallbacks & {
  events: ReturnType<SseConnectionCallbacks["onEvent"] extends (...args: infer A) => any ? () => A[0] : never>[];
  errors: string[];
  doneCalled: number;
} {
  const events: any[] = [];
  const errors: string[] = [];
  let doneCalled = 0;
  return {
    events,
    errors,
    get doneCalled() { return doneCalled; },
    onEvent: (e) => events.push(e),
    onError: (msg) => errors.push(msg),
    onDone: () => doneCalled++,
  };
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("SseConnection — constructor and config", () => {
  afterEach(() => vi.restoreAllMocks());

  it("is initially active (not stopped)", () => {
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      { onEvent: vi.fn(), onError: vi.fn(), onDone: vi.fn() },
    );
    expect(conn.isActive).toBe(true);
  });
});

describe("SseConnection.connect() — POST new stream", () => {
  afterEach(() => vi.restoreAllMocks());

  it("calls fetch with correct URL, method, and Authorization header", async () => {
    mockFetchOk([sseChunk({ type: "finish" })]);
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: { agent: "Alice" }, token: "secret" },
      { onEvent: vi.fn(), onError: vi.fn(), onDone: vi.fn() },
    );
    await conn.connect();
    const [url, init] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0] as [string, RequestInit];
    expect(url).toBe("/api/chat");
    expect(init.method).toBe("POST");
    expect((init.headers as Record<string, string>)["Authorization"]).toBe("Bearer secret");
  });

  it("sends body as JSON for POST requests", async () => {
    mockFetchOk([sseChunk({ type: "finish" })]);
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: { agent: "Alice", messages: [] }, token: "tok" },
      { onEvent: vi.fn(), onError: vi.fn(), onDone: vi.fn() },
    );
    await conn.connect();
    const [, init] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0] as [string, RequestInit];
    expect(init.body).toBe(JSON.stringify({ agent: "Alice", messages: [] }));
    expect((init.headers as Record<string, string>)["Content-Type"]).toBe("application/json");
  });

  it("dispatches parsed SSE events to onEvent callback in order", async () => {
    const cbs = makeCallbacks();
    mockFetchOk([
      sseChunk({ type: "start", messageId: "m1" }),
      sseChunk({ type: "text-delta", delta: "hello" }),
      sseChunk({ type: "text-end" }),
      sseChunk({ type: "finish" }),
    ]);
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      cbs,
    );
    await conn.connect();

    expect(cbs.events.length).toBe(4);
    expect(cbs.events[0].type).toBe("start");
    expect(cbs.events[1].type).toBe("text-delta");
    expect(cbs.events[2].type).toBe("text-end");
    expect(cbs.events[3].type).toBe("finish");
  });

  it("dispatches multiple rapid text-delta events without loss", async () => {
    const cbs = makeCallbacks();
    const deltas = Array.from({ length: 20 }, (_, i) => sseChunk({ type: "text-delta", delta: `d${i}` }));
    mockFetchOk(deltas);
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      cbs,
    );
    await conn.connect();
    const deltaEvents = cbs.events.filter(e => e.type === "text-delta");
    expect(deltaEvents.length).toBe(20);
    expect(cbs.doneCalled).toBe(1);
  });

  it("calls onDone when stream finishes naturally", async () => {
    const cbs = makeCallbacks();
    mockFetchOk([sseChunk({ type: "finish" })]);
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      cbs,
    );
    await conn.connect();
    expect(cbs.doneCalled).toBe(1);
    expect(cbs.errors.length).toBe(0);
  });

  it("calls onError with error text on non-ok HTTP response", async () => {
    const cbs = makeCallbacks();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response("Unauthorized", { status: 401 }),
    );
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "bad" },
      cbs,
    );
    await conn.connect();
    expect(cbs.errors.length).toBe(1);
    expect(cbs.errors[0]).toContain("Unauthorized");
    expect(cbs.doneCalled).toBe(0);
  });

  it("calls onError with HTTP status message when response body is empty", async () => {
    const cbs = makeCallbacks();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response("", { status: 500 }),
    );
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      cbs,
    );
    await conn.connect();
    expect(cbs.errors.length).toBe(1);
    expect(cbs.errors[0]).toContain("500");
  });

  it("skips [DONE] sentinel without calling onError", async () => {
    const cbs = makeCallbacks();
    mockFetchOk([
      sseChunk({ type: "text-delta", delta: "hi" }),
      "data: [DONE]\n",
    ]);
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      cbs,
    );
    await conn.connect();
    expect(cbs.errors.length).toBe(0);
    expect(cbs.events[0].type).toBe("text-delta");
  });

  it("ignores non-data lines and malformed events", async () => {
    const cbs = makeCallbacks();
    mockFetchOk([
      "event: ping\n",
      ": heartbeat\n",
      "data: not-json\n",
      sseChunk({ type: "finish" }),
    ]);
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      cbs,
    );
    await conn.connect();
    expect(cbs.errors.length).toBe(0);
    expect(cbs.events.length).toBe(1);
    expect(cbs.events[0].type).toBe("finish");
  });
});

describe("SseConnection.connect() — GET resume stream", () => {
  afterEach(() => vi.restoreAllMocks());

  it("calls GET /api/chat/{sessionId}/stream without a body", async () => {
    mockFetchOk([sseChunk({ type: "finish" })]);
    const conn = new SseConnection(
      { url: "/api/chat/sess-123/stream", method: "GET", token: "tok" },
      { onEvent: vi.fn(), onError: vi.fn(), onDone: vi.fn() },
    );
    await conn.connect();
    const [url, init] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0] as [string, RequestInit];
    expect(url).toBe("/api/chat/sess-123/stream");
    expect(init.method).toBe("GET");
    expect(init.body).toBeUndefined();
  });

  it("calls onDone (not onError) on 204 response", async () => {
    const cbs = makeCallbacks();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(null, { status: 204 }),
    );
    const conn = new SseConnection(
      { url: "/api/chat/sess-abc/stream", method: "GET", token: "tok" },
      cbs,
    );
    await conn.connect();
    expect(cbs.doneCalled).toBe(1);
    expect(cbs.errors.length).toBe(0);
    expect(cbs.events.length).toBe(0);
  });
});

describe("SseConnection.stop()", () => {
  afterEach(() => vi.restoreAllMocks());

  it("sets isActive to false after stop()", () => {
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      { onEvent: vi.fn(), onError: vi.fn(), onDone: vi.fn() },
    );
    expect(conn.isActive).toBe(true);
    conn.stop();
    expect(conn.isActive).toBe(false);
  });

  it("aborts an in-flight fetch on stop()", async () => {
    const onEvent = vi.fn();
    let resolveFetch!: (v: Response) => void;
    vi.spyOn(globalThis, "fetch").mockImplementation(
      (_url: RequestInfo | URL, init?: RequestInit) =>
        new Promise<Response>((resolve) => {
          // Abort immediately cancels the promise
          init?.signal?.addEventListener("abort", () => {
            resolve(new Response(null, { status: 200 }));
          });
          resolveFetch = resolve;
        }),
    );

    const cbs = makeCallbacks();
    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      cbs,
    );
    const connectPromise = conn.connect();
    conn.stop();
    await connectPromise;
    // After abort, no onEvent calls from a stopped connection
    expect(cbs.events.length).toBe(0);
    expect(conn.isActive).toBe(false);
  });

  it("does not call onEvent after stop()", async () => {
    // Use a stream that is slow to produce chunks
    const onEvent = vi.fn();
    let streamController!: ReadableStreamDefaultController<Uint8Array>;
    const slowStream = new ReadableStream<Uint8Array>({
      start(controller) { streamController = controller; },
    });
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(slowStream, { status: 200 }));

    const conn = new SseConnection(
      { url: "/api/chat", method: "POST", body: {}, token: "tok" },
      { onEvent, onError: vi.fn(), onDone: vi.fn() },
    );

    const connectPromise = conn.connect();
    // Stop before any chunks arrive
    conn.stop();
    // Close the stream so the reader.read() loop can exit
    streamController.close();
    await connectPromise;

    expect(onEvent).not.toHaveBeenCalled();
    expect(conn.isActive).toBe(false);
  });
});

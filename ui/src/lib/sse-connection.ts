// ── SseConnection ─────────────────────────────────────────────────────────────
// Pure SSE transport layer extracted from chat-store.ts.
// No React, no Zustand, no Immer — only Web APIs and sse-events helpers.

import { parseSSELines, parseSseEvent, extractSseEventId } from "@/stores/sse-events";
import type { SseEvent } from "@/stores/sse-events";

export type { SseEvent };

// ── Public types ─────────────────────────────────────────────────────────────

export interface SseConnectionConfig {
  /** POST /api/chat for new stream, GET /api/chat/{id}/stream for resume */
  url: string;
  method: "POST" | "GET";
  /** Request body (POST only). Ignored for GET. */
  body?: Record<string, unknown>;
  /** Bearer token for Authorization header. */
  token: string;
  /** Optional external AbortSignal for cooperative cancellation. */
  signal?: AbortSignal;
  /** Maximum reconnect attempts on unexpected stream drop. Default: 3. */
  maxRetries?: number;
}

export type SseConnectionPhase =
  | "connecting"
  | "streaming"
  | "reconnecting"
  | "error"
  | "done";

export interface SseConnectionCallbacks {
  /** Called for every successfully parsed SSE event. */
  onEvent: (event: SseEvent) => void;
  /** Called when the fetch fails (non-ok HTTP status). NOT called on intentional abort. */
  onError: (error: string) => void;
  /** Called when the stream ends naturally (or on 204 resume response). NOT called on error. */
  onDone: () => void;
}

export interface SseConnectionCallbacksWithPhase extends SseConnectionCallbacks {
  /** Called when the connection phase changes. */
  onPhaseChange: (phase: SseConnectionPhase) => void;
}

// ── SseConnection class ───────────────────────────────────────────────────────

export class SseConnection {
  private readonly controller: AbortController;
  private stopped = false;
  private retryCount = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private sessionId: string | null = null;
  private readonly maxRetries: number;
  private receivedFinish = false;
  private lastEventId: string | null = null;

  constructor(
    private readonly config: SseConnectionConfig,
    private readonly callbacks: SseConnectionCallbacks | SseConnectionCallbacksWithPhase,
  ) {
    this.controller = new AbortController();
    this.maxRetries = config.maxRetries ?? 3;
  }

  /** Set the session ID for reconnect (call when data-session-id event arrives). */
  setSessionId(id: string): void {
    this.sessionId = id;
  }

  private notifyPhase(phase: SseConnectionPhase): void {
    const cbs = this.callbacks as SseConnectionCallbacksWithPhase;
    if (typeof cbs.onPhaseChange === "function") {
      cbs.onPhaseChange(phase);
    }
  }

  /**
   * Start the SSE connection.
   * Returns a promise that resolves when the stream ends (naturally or via abort).
   * Never rejects — errors are delivered via callbacks.onError.
   */
  async connect(): Promise<void> {
    const { url, method, body, token, signal: externalSignal } = this.config;

    this.notifyPhase("connecting");

    // Compose internal + optional external abort signals
    let signal: AbortSignal = this.controller.signal;
    if (externalSignal) {
      // AbortSignal.any() is not universally available — wire up manually
      const combined = new AbortController();
      const onAbort = () => combined.abort();
      this.controller.signal.addEventListener("abort", onAbort);
      externalSignal.addEventListener("abort", onAbort);
      signal = combined.signal;
    }

    const headers: Record<string, string> = {
      Authorization: `Bearer ${token}`,
    };
    const init: RequestInit = { method, headers, signal };
    if (method === "POST" && body !== undefined) {
      headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(body);
    }

    let response: Response;
    try {
      response = await fetch(url, init);
    } catch (err: unknown) {
      // AbortError is intentional — do not call onError
      if (err instanceof Error && err.name === "AbortError") return;
      this.callbacks.onError(err instanceof Error ? err.message : String(err));
      return;
    }

    // 204 on resume endpoint: engine already finished, no stream body
    if (response.status === 204) {
      this.callbacks.onDone();
      this.notifyPhase("done");
      return;
    }

    if (!response.ok) {
      const text = await response.text().catch(() => "");
      this.callbacks.onError(text || `HTTP ${response.status}`);
      return;
    }

    if (!response.body) {
      this.callbacks.onError("Response body is null");
      return;
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    const buffer = { current: "" };
    this.receivedFinish = false;
    let firstByteReceived = false;

    try {
      while (true) {
        if (signal.aborted) break;
        const { done, value } = await reader.read();
        if (done) break;
        if (signal.aborted) break;

        // Notify streaming on first byte
        if (!firstByteReceived) {
          firstByteReceived = true;
          this.notifyPhase("streaming");
        }

        const chunk = decoder.decode(value, { stream: true });
        const lines = parseSSELines(chunk, buffer);

        for (const line of lines) {
          if (signal.aborted) break;
          // Track SSE event IDs for Last-Event-ID header on reconnect
          const eid = extractSseEventId(line);
          if (eid !== null) {
            this.lastEventId = eid;
            continue;
          }
          if (!line.startsWith("data:")) continue;
          const raw = line.slice(5).trim();
          if (raw === "[DONE]") continue;

          const event = parseSseEvent(raw);
          if (!event) continue;

          // Track finish event to distinguish natural end from connection drop
          if (event.type === "finish") {
            this.receivedFinish = true;
          }

          if (!this.stopped) {
            this.callbacks.onEvent(event);
          }
        }
      }
    } finally {
      reader.releaseLock();
    }

    if (signal.aborted) return;

    // Stream ended without finish event — connection dropped unexpectedly
    if (!this.receivedFinish && this.sessionId) {
      this.scheduleReconnect();
      return;
    }

    // Natural end (finish event received before stream closed)
    this.callbacks.onDone();
    this.notifyPhase("done");
  }

  private scheduleReconnect(): void {
    if (this.retryCount >= this.maxRetries) {
      this.callbacks.onError("Max reconnect attempts exceeded");
      this.notifyPhase("error");
      return;
    }
    this.notifyPhase("reconnecting");
    const delay = 1000 * Math.pow(2, this.retryCount);
    this.retryCount++;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.retryConnect();
    }, delay);
  }

  private async retryConnect(): Promise<void> {
    if (this.stopped || this.controller.signal.aborted) return;

    this.notifyPhase("connecting");

    const { token } = this.config;
    const url = `/api/chat/${this.sessionId}/stream`;

    const headers: Record<string, string> = { Authorization: `Bearer ${token}` };
    if (this.lastEventId) {
      headers["Last-Event-ID"] = this.lastEventId;
    }

    let response: Response;
    try {
      response = await fetch(url, {
        method: "GET",
        headers,
        signal: this.controller.signal,
      });
    } catch (err: unknown) {
      if (err instanceof Error && err.name === "AbortError") return;
      // Network error — schedule another retry
      this.scheduleReconnect();
      return;
    }

    if (response.status === 204) {
      // Engine already finished — natural completion
      this.callbacks.onDone();
      this.notifyPhase("done");
      return;
    }

    if (response.status === 410) {
      // Stream expired — treat as natural end, let caller fetch history
      this.callbacks.onDone();
      this.notifyPhase("done");
      return;
    }

    if (!response.ok) {
      // Server error — schedule another retry
      this.scheduleReconnect();
      return;
    }

    if (!response.body) {
      this.scheduleReconnect();
      return;
    }

    // Successfully reconnected — reset retry counter
    this.retryCount = 0;
    this.receivedFinish = false;
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    const buffer = { current: "" };
    let firstByteReceived = false;

    try {
      while (true) {
        if (this.controller.signal.aborted) break;
        const { done, value } = await reader.read();
        if (done) break;
        if (this.controller.signal.aborted) break;

        if (!firstByteReceived) {
          firstByteReceived = true;
          this.notifyPhase("streaming");
        }

        const chunk = decoder.decode(value, { stream: true });
        const lines = parseSSELines(chunk, buffer);

        for (const line of lines) {
          if (this.controller.signal.aborted) break;
          // Track SSE event IDs for subsequent reconnects
          const eid = extractSseEventId(line);
          if (eid !== null) {
            this.lastEventId = eid;
            continue;
          }
          if (!line.startsWith("data:")) continue;
          const raw = line.slice(5).trim();
          if (raw === "[DONE]") continue;

          const event = parseSseEvent(raw);
          if (!event) continue;

          if (event.type === "finish") {
            this.receivedFinish = true;
          }

          if (!this.stopped) {
            this.callbacks.onEvent(event);
          }
        }
      }
    } finally {
      reader.releaseLock();
    }

    if (this.controller.signal.aborted) return;

    if (!this.receivedFinish && this.sessionId) {
      // Connection dropped again — schedule another reconnect
      this.scheduleReconnect();
      return;
    }

    this.callbacks.onDone();
    this.notifyPhase("done");
  }

  /** Abort the connection immediately. Subsequent onEvent calls are suppressed. */
  stop(): void {
    this.stopped = true;
    // Clear any pending reconnect timer
    if (this.retryTimer !== null) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    this.controller.abort();
  }

  /** Whether this connection is still active (not stopped or aborted). */
  get isActive(): boolean {
    return !this.stopped;
  }
}

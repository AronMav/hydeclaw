// ── SseConnection ─────────────────────────────────────────────────────────────
// Pure SSE transport layer extracted from chat-store.ts.
// No React, no Zustand, no Immer — only Web APIs and sse-events helpers.

import { parseSSELines, parseSseEvent } from "@/stores/sse-events";
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
}

export interface SseConnectionCallbacks {
  /** Called for every successfully parsed SSE event. */
  onEvent: (event: SseEvent) => void;
  /** Called when the fetch fails (non-ok HTTP status). NOT called on intentional abort. */
  onError: (error: string) => void;
  /** Called when the stream ends naturally (or on 204 resume response). NOT called on error. */
  onDone: () => void;
}

// ── SseConnection class ───────────────────────────────────────────────────────

export class SseConnection {
  private readonly controller: AbortController;
  private stopped = false;

  constructor(
    private readonly config: SseConnectionConfig,
    private readonly callbacks: SseConnectionCallbacks,
  ) {
    this.controller = new AbortController();
  }

  /**
   * Start the SSE connection.
   * Returns a promise that resolves when the stream ends (naturally or via abort).
   * Never rejects — errors are delivered via callbacks.onError.
   */
  async connect(): Promise<void> {
    const { url, method, body, token, signal: externalSignal } = this.config;

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

    try {
      while (true) {
        if (signal.aborted) break;
        const { done, value } = await reader.read();
        if (done) break;
        if (signal.aborted) break;

        const chunk = decoder.decode(value, { stream: true });
        const lines = parseSSELines(chunk, buffer);

        for (const line of lines) {
          if (signal.aborted) break;
          if (!line.startsWith("data:")) continue;
          const raw = line.slice(5).trim();
          if (raw === "[DONE]") continue;

          const event = parseSseEvent(raw);
          if (!event) continue;

          if (!this.stopped) {
            this.callbacks.onEvent(event);
          }
        }
      }
    } finally {
      reader.releaseLock();
    }

    if (!signal.aborted) {
      this.callbacks.onDone();
    }
  }

  /** Abort the connection immediately. Subsequent onEvent calls are suppressed. */
  stop(): void {
    this.stopped = true;
    this.controller.abort();
  }

  /** Whether this connection is still active (not stopped or aborted). */
  get isActive(): boolean {
    return !this.stopped;
  }
}

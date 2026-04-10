// ── streaming-renderer.ts ──────────────────────────────────────────────────
// Factory module encapsulating SSE stream processing, rAF throttling,
// reconnection logic, and per-agent cleanup (MEM-01, PERF-02).

import { parseSSELines, parseSseEvent, parseContentParts } from "@/stores/sse-events";
import { IncrementalParser } from "@/lib/message-parser";
import { apiPatch, assertToken } from "@/lib/api";
import { queryClient } from "@/lib/query-client";
import { qk } from "@/lib/queries";
import type { SessionRow } from "@/types/api";

import {
  uuid,
  STREAM_THROTTLE_MS,
  emptyAgentState,
  getLiveMessages,
} from "./chat-types";
import type {
  ChatMessage,
  MessagePart,
  TextPart,
  ToolPart,
  ConnectionPhase,
  AgentState,
} from "./chat-types";
import { getCachedHistoryMessages } from "./chat-history";

// ── Store access interface ─────────────────────────────────────────────────
// Uses `any` for store shape to avoid circular dependency with ChatStore.

interface StoreAccess {
  get: () => any;
  set: (fn: (draft: any) => void) => void;
}

// ── Reconnect constants (SSE-02) ─────────────────────────────────────────────
const MAX_RECONNECT_ATTEMPTS = 3;
const RECONNECT_DELAY_BASE_MS = 1000;

// ── Factory ────────────────────────────────────────────────────────────────

export function createStreamingRenderer(store: StoreAccess) {
  // ── CLN-02: Encapsulated non-serializable state ──────────────────────────
  // AbortController and setTimeout handles are not plain objects -- Immer cannot
  // proxy or freeze them. They live in private Maps inside the factory closure.

  const _abortControllers = new Map<string, AbortController | null>();
  const _reconnectTimers = new Map<string, ReturnType<typeof setTimeout> | null>();

  function getAbortCtrl(agent: string): AbortController | null {
    return _abortControllers.get(agent) ?? null;
  }
  function setAbortCtrl(agent: string, ctrl: AbortController | null): void {
    _abortControllers.set(agent, ctrl);
  }
  function getReconnectTimer(agent: string): ReturnType<typeof setTimeout> | null {
    return _reconnectTimers.get(agent) ?? null;
  }
  function setReconnectTimer(agent: string, timer: ReturnType<typeof setTimeout> | null): void {
    _reconnectTimers.set(agent, timer);
  }

  // ── Internal helpers ────────────────────────────────────────────────────

  function ensure(agent: string): AgentState {
    const s = store.get().agents[agent];
    if (s) return s;
    const fresh = emptyAgentState();
    store.set((draft: any) => { draft.agents[agent] = fresh; });
    return fresh;
  }

  function update(agent: string, patch: Partial<AgentState>) {
    store.set((draft: any) => {
      if (!draft.agents[agent]) draft.agents[agent] = emptyAgentState();
      Object.assign(draft.agents[agent], patch);
    });
  }

  // ── Debounced UI state persistence to server ──────────────────────────────
  const uiStateSaveTimers: Record<string, ReturnType<typeof setTimeout>> = {};
  function saveUiState(agent: string) {
    clearTimeout(uiStateSaveTimers[agent]);
    uiStateSaveTimers[agent] = setTimeout(() => {
      const st = store.get().agents[agent];
      if (!st?.activeSessionId) return;
      apiPatch(`/api/sessions/${st.activeSessionId}`, {
        ui_state: { connectionPhase: st.connectionPhase },
      }).catch((e: unknown) => { console.warn("[chat] save failed:", e); });
    }, 500);
  }

  // ── Stream lifecycle ────────────────────────────────────────────────────

  /**
   * Resume an active backend stream after page reload.
   * Connects to GET /api/chat/{sessionId}/stream and processes replay + live events.
   */
  function resumeStream(agent: string, sessionId: string, reconnectAttempt = 0) {
    // Don't resume if already streaming (but allow reconnect path even in "reconnecting" phase)
    const st = store.get().agents[agent];
    if (st && st.connectionPhase === "streaming") return;

    // Clear any existing reconnect timer before starting a new stream
    const existingTimer = getReconnectTimer(agent);
    if (existingTimer) {
      clearTimeout(existingTimer);
      setReconnectTimer(agent, null);
    }
    abortActiveStream(agent);
    update(agent, { streamGeneration: (store.get().agents[agent]?.streamGeneration ?? 0) + 1 });
    const myGeneration = store.get().agents[agent]?.streamGeneration ?? 1;
    const controller = new AbortController();
    setAbortCtrl(agent, controller);

    // Seed with history from React Query cache so UI shows messages immediately (Fix A).
    const existingSt = store.get().agents[agent];
    const seedMessages = existingSt?.messageSource.mode === "live"
      ? existingSt.messageSource.messages
      : getCachedHistoryMessages(sessionId);

    update(agent, {
      streamError: null,
      connectionPhase: "streaming",
      connectionError: null,
      messageSource: { mode: "live", messages: seedMessages },
    });

    const token = assertToken();

    fetch(`/api/chat/${sessionId}/stream`, {
      method: "GET",
      headers: { Authorization: `Bearer ${token}` },
      signal: controller.signal,
    })
      .then((resp) => {
        if (resp.status === 204) {
          // No active stream -- engine already finished. Switch to history and refetch.
          update(agent, { connectionPhase: "idle", messageSource: { mode: "history", sessionId } });
          queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
          queryClient.invalidateQueries({ queryKey: qk.sessionMessages(sessionId) });
          return;
        }
        if (!resp.ok) {
          return resp.text().then((t) => { throw new Error(t || `HTTP ${resp.status}`); });
        }
        return processSSEStream(agent, resp.body!, controller.signal, myGeneration, sessionId, reconnectAttempt);
      })
      .catch((err) => {
        if (err.name === "AbortError") return;
        // Network error during reconnect -- schedule next retry
        if (reconnectAttempt < MAX_RECONNECT_ATTEMPTS) {
          scheduleReconnect(agent, sessionId, reconnectAttempt);
        } else {
          update(agent, { connectionPhase: "idle" });
        }
      });
  }

  /** Abort active stream for an agent and reset status. */
  function abortActiveStream(agent: string) {
    // Clear any pending reconnect timer first -- prevents reconnect after abort
    const timer = getReconnectTimer(agent);
    if (timer) {
      clearTimeout(timer);
      setReconnectTimer(agent, null);
    }
    const ctrl = getAbortCtrl(agent);
    if (ctrl) {
      ctrl.abort();
      setAbortCtrl(agent, null);
      update(agent, { connectionPhase: "idle" });
    }
  }

  // ── Reconnect scheduling (SSE-02) ────────────────────────────────────────
  function scheduleReconnect(agent: string, sessionId: string, attempt: number) {
    if (attempt >= MAX_RECONNECT_ATTEMPTS) {
      const sid = sessionId ?? store.get().agents[agent]?.activeSessionId;
      update(agent, {
        streamError: "Connection lost after retries",
        connectionPhase: "error",
        connectionError: "Connection lost after retries",
        messageSource: sid ? { mode: "history", sessionId: sid } : { mode: "new-chat" },
      });
      return;
    }
    update(agent, { connectionPhase: "reconnecting", connectionError: null });
    const baseDelay = RECONNECT_DELAY_BASE_MS * Math.pow(2, attempt);
    const jitter = baseDelay * 0.2 * (Math.random() * 2 - 1); // +/- 20% jitter
    const delay = Math.max(0, baseDelay + jitter);
    setReconnectTimer(agent, setTimeout(() => {
      setReconnectTimer(agent, null);
      resumeStream(agent, sessionId, attempt + 1);
    }, delay));
  }

  // ── SSE stream handler ──────────────────────────────────────────────────

  function startStream(agent: string, sessionId: string | null, messages: ChatMessage[], userText: string) {
    abortActiveStream(agent);
    update(agent, { streamGeneration: (store.get().agents[agent]?.streamGeneration ?? 0) + 1 });
    const myGeneration = store.get().agents[agent]?.streamGeneration ?? 1;
    const controller = new AbortController();
    setAbortCtrl(agent, controller);

    // Build user message -- optimistic status: "sending" until data-session-id confirms receipt
    const userMsg: ChatMessage = {
      id: uuid(),
      role: "user",
      parts: [{ type: "text", text: userText }],
      createdAt: new Date().toISOString(),
      status: "sending",
    };
    const allMessages = [...messages, userMsg];
    update(agent, {
      messageSource: { mode: "live", messages: allMessages },
      streamError: null,
      connectionPhase: "submitted",
      connectionError: null,
      agentTurns: [],  // Reset for new stream
      turnCount: 0,
      turnLimitMessage: null,
      pendingTargetAgent: null,  // clear stale target from previous stream
    });
    saveUiState(agent);

    // Build request body -- backend only uses the last user message + session_id
    const agentState = store.get().agents[agent];
    const forceNew = agentState?.forceNewSession ?? false;
    const body: Record<string, unknown> = {
      agent,
      messages: [{ role: "user", content: userText }],
    };
    if (sessionId) body.session_id = sessionId;
    if (forceNew) {
      body.force_new_session = true;
      update(agent, { forceNewSession: false });
    }

    const token = assertToken();

    fetch("/api/chat", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${token}`,
      },
      body: JSON.stringify(body),
      signal: controller.signal,
    })
      .then((resp) => {
        if (!resp.ok) {
          return resp.text().then((t) => {
            throw new Error(t || `HTTP ${resp.status}`);
          });
        }
        return processSSEStream(agent, resp.body!, controller.signal, myGeneration);
      })
      .catch((err) => {
        if (err.name === "AbortError") return;
        const errMsg = err.message || "Stream failed";
        // SSE-03: Mark the optimistic user message as failed so the UI shows an error indicator.
        store.set((draft: any) => {
          const st = draft.agents[agent];
          if (!st || st.messageSource.mode !== "live") return;
          const msgs = st.messageSource.messages;
          for (let i = msgs.length - 1; i >= 0; i--) {
            if (msgs[i].role === "user" && msgs[i].status === "sending") {
              msgs[i].status = "failed";
              break;
            }
          }
        });
        update(agent, {
          streamError: errMsg,
          connectionPhase: "error",
          connectionError: errMsg,
        });
        saveUiState(agent);
      });
  }

  async function processSSEStream(
    agent: string,
    body: ReadableStream<Uint8Array>,
    signal: AbortSignal,
    generation: number,
    knownSessionId?: string,
    reconnectAttempt = 0,
  ) {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    const buffer = { current: "" };

    // Mutable assistant message being built
    let assistantId = uuid();
    let assistantCreatedAt = new Date().toISOString();
    let parts: MessagePart[] = [];
    const incrementalParser = new IncrementalParser();
    const toolInputChunks = new Map<string, string[]>();
    let receivedSessionId: string | null = knownSessionId ?? null;
    // Track finish event to distinguish natural end from connection drop
    let receivedFinishEvent = false;
    // Initialize from pendingTargetAgent so first render shows correct avatar.
    let currentRespondingAgent: string | null = store.get().agents[agent]?.pendingTargetAgent ?? agent;

    function flushText() {
      const flushed = incrementalParser.flush();
      if (flushed.length > 0) {
        parts.push(...flushed);
      }
    }

    function pushUpdate() {
      // Guard: stale stream -- a newer stream has started, discard updates
      if (generation !== (store.get().agents[agent]?.streamGeneration ?? 0)) return;
      // Guard: don't update store after abort (prevents race with stopStream)
      if (signal.aborted) return;

      const textParts = incrementalParser.snapshot();
      const nonTextParts = parts.filter(p => p.type !== "text" && p.type !== "reasoning");

      store.set((draft: any) => {
        const st = draft.agents[agent];
        if (!st) return;
        if (st.messageSource.mode !== "live") {
          st.messageSource = { mode: "live", messages: [] };
        }
        const liveMessages = st.messageSource.messages;
        const existing = liveMessages.findIndex((m: ChatMessage) => m.id === assistantId);

        const allParts = [...textParts, ...nonTextParts] as MessagePart[];

        if (existing >= 0) {
          const msg = liveMessages[existing];
          msg.parts = allParts;
          msg.agentId = currentRespondingAgent ?? undefined;
        } else {
          liveMessages.push({
            id: assistantId,
            role: "assistant",
            parts: allParts,
            createdAt: assistantCreatedAt,
            agentId: currentRespondingAgent ?? undefined,
          });
        }
        if (st.connectionPhase !== "error") st.connectionPhase = "streaming";
      });
    }

    // Throttle UI updates to ~20fps (50ms) -- reduces renders from 60/sec to 20/sec.
    // setTimeout coalesces rapid SSE events, then rAF syncs with browser paint cycle.
    let updateScheduled = false;
    let updateTimer: ReturnType<typeof setTimeout> | null = null;
    function scheduleUpdate() {
      if (updateScheduled) return;
      updateScheduled = true;
      updateTimer = setTimeout(() => {
        updateTimer = null;
        requestAnimationFrame(() => {
          updateScheduled = false;
          pushUpdate();
        });
      }, STREAM_THROTTLE_MS);
    }
    function cancelScheduledUpdate() {
      if (updateTimer) { clearTimeout(updateTimer); updateTimer = null; }
      updateScheduled = false;
    }

    try {
      while (true) {
        if (signal.aborted) break;
        const { done, value } = await reader.read();
        if (done) break;

        const chunk = decoder.decode(value, { stream: true });
        const lines = parseSSELines(chunk, buffer);

        for (const line of lines) {
          if (!line.startsWith("data:")) continue;
          const raw = line.slice(5).trim();
          if (raw === "[DONE]") continue;

          const event = parseSseEvent(raw);
          if (!event) continue;

          switch (event.type) {
            case "data-session-id": {
              const sid = event.data.sessionId;
              if (sid && generation === (store.get().agents[agent]?.streamGeneration ?? 0)) {
                receivedSessionId = sid;
                // SSE-03: Confirm the optimistic user message
                store.set((draft: any) => {
                  const st = draft.agents[agent];
                  if (!st || st.messageSource.mode !== "live") return;
                  const msgs = st.messageSource.messages;
                  for (let i = msgs.length - 1; i >= 0; i--) {
                    if (msgs[i].role === "user" && msgs[i].status === "sending") {
                      msgs[i].status = "confirmed";
                      break;
                    }
                  }
                });
                update(agent, { activeSessionId: sid });
                // saveLastSession is called from chat-store.ts via the callback
                _onSessionId?.(agent, sid);

                // Populate sessionParticipants cache from React Query session data
                const sessionsData = queryClient.getQueryData<{ sessions: SessionRow[] }>(
                  qk.sessions(agent)
                );
                const session = sessionsData?.sessions.find(s => s.id === sid);
                if (session?.participants) {
                  store.get().updateSessionParticipants(sid, session.participants);
                }
              }
              break;
            }

            case "start": {
              const newId = event.messageId || assistantId;
              assistantId = newId;
              assistantCreatedAt = new Date().toISOString();
              parts = [];
              if (event.agentName) currentRespondingAgent = event.agentName;
              // Dedup: remove resume placeholder and seeded message with same ID
              const stNow = store.get().agents[agent];
              if (stNow && stNow.messageSource.mode === "live") {
                const currentMessages = stNow.messageSource.messages;
                const deduped = currentMessages.filter(
                  (m: ChatMessage) => m.id !== newId && !m.id.startsWith("resume-")
                );
                if (deduped.length !== currentMessages.length) {
                  update(agent, { messageSource: { mode: "live", messages: deduped } });
                }
              }
              break;
            }

            case "text-start": {
              if (event.agentName) currentRespondingAgent = event.agentName;
              break;
            }

            case "text-delta": {
              incrementalParser.processDelta(event.delta);
              scheduleUpdate();
              break;
            }

            case "text-end": {
              scheduleUpdate();
              break;
            }

            case "tool-input-start": {
              flushText();
              const { toolCallId: tcId, toolName: tcName } = event;
              toolInputChunks.set(tcId, []);
              parts.push({
                type: "tool",
                toolCallId: tcId,
                toolName: tcName,
                state: "input-streaming",
                input: {},
              });
              scheduleUpdate();
              break;
            }

            case "tool-input-delta": {
              const { toolCallId: tcId, inputTextDelta: delta } = event;
              if (delta) toolInputChunks.get(tcId)?.push(delta);
              break;
            }

            case "tool-input-available": {
              const { toolCallId: tcId, input } = event;
              toolInputChunks.delete(tcId);
              const idx = parts.findIndex(
                (p) => p.type === "tool" && p.toolCallId === tcId,
              );
              if (idx >= 0) {
                parts[idx] = { ...(parts[idx] as ToolPart), state: "input-available", input: (input as Record<string, unknown>) ?? {} };
              }
              scheduleUpdate();
              break;
            }

            case "tool-output-available": {
              const { toolCallId: tcId, output } = event;
              const idx = parts.findIndex(
                (p) => p.type === "tool" && p.toolCallId === tcId,
              );
              if (idx >= 0) {
                parts[idx] = { ...(parts[idx] as ToolPart), state: "output-available", output };
              }
              scheduleUpdate();
              break;
            }

            case "file": {
              flushText();
              parts.push({
                type: "file",
                url: event.url,
                mediaType: event.mediaType || "application/octet-stream",
              });
              scheduleUpdate();
              break;
            }

            case "rich-card": {
              flushText();
              if (event.cardType === "agent-turn" && event.data?.agentName) {
                currentRespondingAgent = event.data.agentName as string;
                const currentTurnCount = store.get().agents[agent]?.turnCount ?? 0;
                update(agent, { pendingTargetAgent: currentRespondingAgent, turnCount: currentTurnCount + 1 });
                break;
              }
              parts.push({
                type: "rich-card",
                cardType: event.cardType,
                data: event.data,
              });
              scheduleUpdate();
              break;
            }

            case "sync": {
              const { content: syncContent, toolCalls: syncToolCalls, status: syncStatus } = event;
              const normalizedParts = parseContentParts(syncContent || "");

              const syncParts: MessagePart[] = [...normalizedParts];
              for (const tc of syncToolCalls as Array<Record<string, unknown>>) {
                syncParts.push({
                  type: "tool",
                  toolCallId: (tc.toolCallId as string) ?? "",
                  toolName: (tc.toolName as string) ?? "tool",
                  state: "output-available",
                  input: {},
                  output: tc.output,
                });
              }

              store.set((draft: any) => {
                const st = draft.agents[agent];
                if (!st) return;
                if (st.messageSource.mode !== "live") {
                  st.messageSource = { mode: "live", messages: [] };
                }
                const liveMessages = st.messageSource.messages;
                const existingIdx = liveMessages.findIndex((m: ChatMessage) => m.id === assistantId);
                if (existingIdx >= 0) {
                  liveMessages[existingIdx].parts = syncParts;
                } else {
                  const userMsgs = liveMessages.filter((m: ChatMessage) => m.role === "user");
                  st.messageSource = { mode: "live", messages: [...userMsgs, {
                    id: assistantId,
                    role: "assistant",
                    parts: syncParts,
                    createdAt: assistantCreatedAt,
                    agentId: currentRespondingAgent ?? undefined,
                  }] };
                }
              });

              if (syncStatus === "finished" || syncStatus === "error" || syncStatus === "interrupted") {
                const errorText = syncStatus === "error" ? (event.error ?? null) : null;
                const inTurnLoop = !!store.get().agents[agent]?.pendingTargetAgent;
                if (syncStatus === "error" || !inTurnLoop) {
                  const newPhase: ConnectionPhase = syncStatus === "error" ? "error" : "idle";
                  update(agent, {
                    streamError: errorText,
                    connectionPhase: newPhase,
                    connectionError: errorText,
                  });
                }
              }
              break;
            }

            case "finish": {
              receivedFinishEvent = true;
              cancelScheduledUpdate();
              flushText();
              pushUpdate();
              incrementalParser.reset();
              assistantId = uuid();
              assistantCreatedAt = new Date().toISOString();
              parts = [];
              break;
            }

            case "error": {
              const errText = event.errorText;
              if (errText.includes("turn limit") || errText.includes("cycle detected")) {
                update(agent, { turnLimitMessage: errText, turnCount: 0 });
              } else {
                update(agent, {
                  streamError: errText,
                  connectionPhase: "error",
                  connectionError: errText,
                });
              }
              break;
            }
          }
        }
      }
    } finally {
      reader.releaseLock();
      if (updateScheduled) {
        cancelScheduledUpdate();
        pushUpdate();
      }
      flushText();
      if (!signal.aborted) {
        if (parts.length > 0) pushUpdate();

        // SSE-02: Detect connection drop (stream ended without finish event).
        const isError = store.get().agents[agent]?.connectionPhase === "error";
        if (!isError && !receivedFinishEvent && receivedSessionId) {
          scheduleReconnect(agent, receivedSessionId, reconnectAttempt);
          return;
        }

        if (!isError) {
          update(agent, {
            connectionPhase: "idle",
            connectionError: null,
            pendingTargetAgent: null,
            turnCount: 0,
          });
        }
        saveUiState(agent);
      } else if (parts.length > 0) {
        const st = store.get().agents[agent];
        if (st) {
          const assistantMsg: ChatMessage = {
            id: assistantId,
            role: "assistant",
            parts: [...parts],
            createdAt: assistantCreatedAt,
            agentId: currentRespondingAgent ?? undefined,
          };
          const currentMessages = getLiveMessages(st.messageSource);
          const existing = currentMessages.findIndex((m: ChatMessage) => m.id === assistantId);
          const updated =
            existing >= 0
              ? currentMessages.map((m: ChatMessage, i: number) => (i === existing ? assistantMsg : m))
              : [...currentMessages, assistantMsg];
          update(agent, { messageSource: { mode: "live", messages: updated } });
        }
      }
    }

    // Save and invalidate React Query caches
    if (!signal.aborted) {
      if (receivedSessionId) {
        _onSessionId?.(agent, receivedSessionId);
      }
      queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
      const completedSessionId = receivedSessionId ?? store.get().agents[agent]?.activeSessionId;
      if (completedSessionId) {
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(completedSessionId) });
      }
    }
  }

  // ── Callback for saveLastSession (avoids circular import) ─────────────
  let _onSessionId: ((agent: string, sessionId: string) => void) | null = null;

  // ── MEM-01: Agent cleanup ──────────────────────────────────────────────

  function cleanupAgent(agent: string) {
    const ctrl = _abortControllers.get(agent);
    if (ctrl) ctrl.abort();
    _abortControllers.delete(agent);
    const timer = _reconnectTimers.get(agent);
    if (timer) clearTimeout(timer);
    _reconnectTimers.delete(agent);
    // Clean up debounce timers
    clearTimeout(uiStateSaveTimers[agent]);
    delete uiStateSaveTimers[agent];
  }

  // ── Public API ─────────────────────────────────────────────────────────

  return {
    startStream,
    resumeStream,
    abortActiveStream,
    cleanupAgent,
    isAgentStreaming: (agent: string) => _abortControllers.has(agent) && _abortControllers.get(agent) !== null,
    getAbortCtrl,
    getReconnectTimer,
    setReconnectTimer,
    /** Register callback for session ID events (called with agent, sessionId). */
    onSessionId(cb: (agent: string, sessionId: string) => void) {
      _onSessionId = cb;
    },
  };
}

export type StreamingRenderer = ReturnType<typeof createStreamingRenderer>;

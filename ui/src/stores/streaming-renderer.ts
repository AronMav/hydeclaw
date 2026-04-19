// ── streaming-renderer.ts ──────────────────────────────────────────────────
// Factory module encapsulating SSE stream processing, rAF throttling,
// reconnection logic, and per-agent cleanup (MEM-01, PERF-02).

import { parseSSELines, parseSseEvent, parseContentParts } from "@/stores/sse-events";
import { IncrementalParser } from "@/lib/message-parser";
import { apiPatch, apiPost, assertToken } from "@/lib/api";
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
  ApprovalPart,

  ConnectionPhase,
  AgentState,
} from "./chat-types";
import { getCachedRawMessages, resolveActivePath } from "./chat-history";

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
    // Local-only cleanup: DO NOT POST /abort here. The previous stream on
    // the same session id may have already ended, and if we POST /abort
    // during startup, the backend cancels the stream we are about to start
    // (same session id → same cancel token).
    abortLocalOnly(agent);
    update(agent, { streamGeneration: (store.get().agents[agent]?.streamGeneration ?? 0) + 1 });
    const myGeneration = store.get().agents[agent]?.streamGeneration ?? 1;
    const controller = new AbortController();
    setAbortCtrl(agent, controller);

    // Architecture C: live messages = overlay only (current streaming message).
    // History comes from React Query. No seed needed.
    update(agent, {
      streamError: null,
      connectionPhase: "streaming",
      connectionError: null,
      messageSource: { mode: "live" as const, messages: [] },
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
        if (resp.status === 401) {
          import("@/lib/api").then(({ handleUnauthorized }) => handleUnauthorized());
          return;
        }
        if (!resp.ok) {
          return resp.text().then((t) => { throw new Error(t || `HTTP ${resp.status}`); });
        }
        return processSSEStream(agent, resp.body!, controller.signal, myGeneration, sessionId, reconnectAttempt);
      })
      .catch((err) => {
        if (err.name === "AbortError") return;
        // Guard: if a newer stream started, don't schedule reconnect for the old one
        if (myGeneration !== (store.get().agents[agent]?.streamGeneration ?? 0)) return;
        // Network error during reconnect -- schedule next retry
        if (reconnectAttempt < MAX_RECONNECT_ATTEMPTS) {
          scheduleReconnect(agent, sessionId, reconnectAttempt);
        } else {
          update(agent, { connectionPhase: "idle" });
        }
      });
  }

  /** Internal: local abort only (no backend notification). Used by
   * startStream to clean up lingering fetch controllers before launching
   * a new stream on the same agent. Calling /abort here would race with
   * the new stream's registration on the same session id and cancel it
   * prematurely.
   */
  function abortLocalOnly(agent: string) {
    const timer = getReconnectTimer(agent);
    if (timer) {
      clearTimeout(timer);
      setReconnectTimer(agent, null);
    }
    const ctrl = getAbortCtrl(agent);
    if (ctrl) {
      ctrl.abort();
      setAbortCtrl(agent, null);
      // Bump streamGeneration so in-flight SSE chunks that arrive AFTER
      // `ctrl.abort()` but before the read loop sees `signal.aborted`
      // are dismissed by the existing `generation === currentGeneration`
      // guards (see pushUpdate + data-session-id handler). Without this,
      // a lagging `data-session-id` event clobbers `activeSessionId`
      // right after the user switched sessions — the new session's
      // text then appears not to load. This bump matters more since
      // navigation uses abortLocalOnly (no backend cancel → more
      // in-flight chunks racing the abort).
      const currentGen = store.get().agents[agent]?.streamGeneration ?? 0;
      update(agent, {
        connectionPhase: "idle",
        streamGeneration: currentGen + 1,
      });
    }
  }

  /** Public: abort active stream AND notify backend (user Stop).
   *
   * Fire-and-forget POST /api/chat/{sid}/abort trips the backend's
   * CancellationToken, which cascades through `stream_with_cancellation`
   * into `LlmCallError::UserCancelled { partial_text }`. The engine's
   * error path then persists an aborted message row with
   * `abort_reason='user_cancelled'` and writes an aborted usage_log.
   *
   * The /abort POST fires whenever an `activeSessionId` is known, even
   * if the local AbortController is already gone (network tear-down,
   * SSE auto-reconnect race). This matters because the backend stream
   * may still be registered under the sessionId while the UI has
   * already disposed of its fetch — without this decoupling, user Stop
   * becomes a silent no-op server-side and the streaming row stays
   * `status='streaming'` until the engine finishes naturally.
   *
   * `abortLocalOnly` is a no-op if there is no controller; safe to call.
   */
  function abortActiveStream(agent: string) {
    const sid = store.get().agents[agent]?.activeSessionId;
    if (sid) {
      apiPost(`/api/chat/${sid}/abort`).catch(() => {
        // Backend may not have an active stream (already done / not started).
        // Local abort below still cleans up UI state.
      });
    }
    abortLocalOnly(agent);
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
    update(agent, { connectionPhase: "reconnecting", connectionError: null, reconnectAttempt: attempt + 1 });
    const baseDelay = RECONNECT_DELAY_BASE_MS * Math.pow(2, attempt);
    const jitter = baseDelay * 0.2 * (Math.random() * 2 - 1); // +/- 20% jitter
    const delay = Math.max(0, baseDelay + jitter);
    setReconnectTimer(agent, setTimeout(() => {
      setReconnectTimer(agent, null);
      resumeStream(agent, sessionId, attempt + 1);
    }, delay));
  }

  // ── SSE stream handler ──────────────────────────────────────────────────

  function startStream(agent: string, sessionId: string | null, messages: ChatMessage[], userText: string, attachments?: Array<any>) {
    // Local-only cleanup for the same reason documented in resumeStream.
    abortLocalOnly(agent);
    update(agent, { streamGeneration: (store.get().agents[agent]?.streamGeneration ?? 0) + 1 });
    const myGeneration = store.get().agents[agent]?.streamGeneration ?? 1;
    const controller = new AbortController();
    setAbortCtrl(agent, controller);

    const userParts: MessagePart[] = [];
    if (userText) userParts.push({ type: "text", text: userText });
    
    const apiAttachments: any[] = [];
    if (attachments && attachments.length > 0) {
      for (const att of attachments) {
        for (const content of att.content) {
          userParts.push({
            type: "file",
            url: content.data,
            mediaType: content.mimeType,
          });
          
          apiAttachments.push({
            url: content.data,
            media_type: content.mimeType.startsWith("image/") ? "image" : "document",
            file_name: content.filename ?? att.name,
            mime_type: content.mimeType,
          });
        }
      }
    }
    
    if (userParts.length === 0) {
      userParts.push({ type: "text", text: "" });
    }

    // Build user message -- optimistic status: "sending" until data-session-id confirms receipt
    const userMsg: ChatMessage = {
      id: uuid(),
      role: "user",
      parts: userParts,
      createdAt: new Date().toISOString(),
      status: "sending",
    };
    // Architecture C: live = overlay only. History provides past messages.
    // Overlay contains just the optimistic user message (until history picks it up).
    update(agent, {
      messageSource: { mode: "live", messages: [userMsg] },
      streamError: null,
      connectionPhase: "submitted",
      connectionError: null,
      turnLimitMessage: null,
    });
    saveUiState(agent);

    // Build request body -- backend only uses the last user message + session_id
    const agentState = store.get().agents[agent];
    const forceNew = agentState?.forceNewSession ?? false;
    const body: Record<string, unknown> = {
      agent,
      messages: [{ role: "user", content: userText }],
    };
    if (apiAttachments.length > 0) {
      body.attachments = apiAttachments;
    }
    if (sessionId) {
      body.session_id = sessionId;
      // Send leaf_message_id — the tip of the currently viewed branch.
      // Use resolveActivePath to find the correct leaf (not the absolute last message,
      // which could be on a different branch).
      const rawMsgs = getCachedRawMessages(sessionId);
      if (rawMsgs.length > 0) {
        const agentSt = store.get().agents[agent];
        const branches = agentSt?.selectedBranches ?? {};
        const hasBranching = rawMsgs.some(m => m.parent_message_id != null);
        if (hasBranching) {
          const activePath = resolveActivePath(rawMsgs, branches);
          if (activePath.length > 0) {
            body.leaf_message_id = activePath[activePath.length - 1].id;
          }
        } else {
          body.leaf_message_id = rawMsgs[rawMsgs.length - 1].id;
        }
      }
    }
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
        if (resp.status === 401) {
          import("@/lib/api").then(({ handleUnauthorized }) => handleUnauthorized());
          return;
        }
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
    let currentRespondingAgent: string | null = agent;

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
        // Double-check generation inside draft to close race window
        if (st.streamGeneration !== generation) return;
        if (st.messageSource.mode !== "live") {
          st.messageSource = { mode: "live", messages: [] };
        }
        // Preserve existing overlay messages (e.g. optimistic user msg)
        const liveMessages = st.messageSource.messages;
        const existing = liveMessages.findIndex((m: ChatMessage) => m.id === assistantId);

        const allParts = [...nonTextParts, ...textParts] as MessagePart[];

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
          if (!event) {
            if (process.env.NODE_ENV !== "production") console.warn("[sse] unparseable event:", raw.slice(0, 120));
            continue;
          }

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

              // Don't reset if current message only has tool/approval parts —
              // Architecture C: each `start` = new LLM iteration.
              // Reset overlay for new assistant message. History refresh happens
              // via React Query polling (3-5s) — no forced invalidation to avoid races.
              assistantId = newId;
              assistantCreatedAt = new Date().toISOString();
              parts = [];
              incrementalParser.reset();
              if (event.agentName) currentRespondingAgent = event.agentName;
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
              const toolPart: ToolPart = {
                type: "tool",
                toolCallId: tcId,
                toolName: tcName,
                state: "input-streaming",
                input: {},
              };
              parts.push(toolPart);
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

            case "tool-approval-needed": {
              flushText();
              const approval: ApprovalPart = {
                type: "approval",
                approvalId: event.approvalId,
                toolName: event.toolName,
                toolInput: event.toolInput,
                timeoutMs: event.timeoutMs,
                receivedAt: Date.now(),
                status: "pending",
              };
              parts.push(approval);
              scheduleUpdate();
              break;
            }

            case "tool-approval-resolved": {
              const idx = parts.findIndex(
                (p) => p.type === "approval" && p.approvalId === event.approvalId,
              );
              if (idx >= 0) {
                const existing = parts[idx] as ApprovalPart;
                parts[idx] = {
                  ...existing,
                  status: event.action,
                  modifiedInput: event.modifiedInput,
                };
              }
              scheduleUpdate();
              break;
            }

            case "step-start":
            case "step-finish":
              // Step groups removed — tools render as flat parts (matching history view)
              break;

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
              parts.push({
                type: "rich-card",
                cardType: event.cardType,
                data: event.data,
              });
              scheduleUpdate();
              break;
            }

            case "sync": {
              const { content: syncContent, status: syncStatus } = event;
              // Architecture C: overlay = text only. Tools come from history (DB).
              const syncParts: MessagePart[] = parseContentParts(syncContent || "");

              store.set((draft: any) => {
                const st = draft.agents[agent];
                if (!st) return;
                // Guard: skip sync events for a different session (e.g. agent switch race)
                if (receivedSessionId && st.activeSessionId && receivedSessionId !== st.activeSessionId) return;

                const currentSessionId = st.activeSessionId;
                const isSameSession = receivedSessionId && currentSessionId === receivedSessionId;

                if (st.messageSource.mode !== "live" && !isSameSession) {
                  st.messageSource = { mode: "live", messages: [] };
                }
                
                const liveMessages = st.messageSource.messages;
                const existingIdx = liveMessages.findIndex((m: ChatMessage) => m.id === assistantId);
                
                if (existingIdx >= 0) {
                  const existingMsg = liveMessages[existingIdx];
                  // Merge content: keep local text if it's ahead of sync (prevents flicker)
                  // but accept sync if it's significantly different (recon from scratch)
                  const localTextLen = existingMsg.parts
                    .filter((p: MessagePart): p is TextPart => p.type === "text")
                    .reduce((acc: number, p: TextPart) => acc + (p.text?.length ?? 0), 0);
                  const syncTextLen = syncParts
                    .filter((p: MessagePart): p is TextPart => p.type === "text")
                    .reduce((acc: number, p: TextPart) => acc + (p.text?.length ?? 0), 0);

                  if (syncTextLen > localTextLen || Math.abs(syncTextLen - localTextLen) > 50) {
                     existingMsg.parts = syncParts;
                  }
                  
                  if (existingMsg.status !== "complete") {
                    existingMsg.status = (syncStatus === "done" || syncStatus === "finished") ? "complete" : "streaming";
                  }
                } else {
                  liveMessages.push({
                    id: assistantId,
                    role: "assistant",
                    parts: syncParts,
                    createdAt: assistantCreatedAt,
                    agentId: currentRespondingAgent ?? undefined,
                    status: (syncStatus === "done" || syncStatus === "finished") ? "complete" : "streaming",
                  });
                }
                
                if (st.connectionPhase !== "error" && syncStatus !== "done" && syncStatus !== "finished") {
                  st.connectionPhase = "streaming";
                } else if (syncStatus === "done" || syncStatus === "finished") {
                  st.connectionPhase = "idle";
                }
              });

              if (syncStatus === "finished" || syncStatus === "error" || syncStatus === "interrupted") {
                const errorText = syncStatus === "error" ? (event.error ?? null) : null;
                const newPhase: ConnectionPhase = syncStatus === "error" ? "error" : "idle";
                update(agent, {
                  streamError: errorText,
                  connectionPhase: newPhase,
                  connectionError: errorText,
                });
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

              // Mark session as no longer running (don't rely solely on WS agent_processing event)
              if (receivedSessionId) {
                const sid = receivedSessionId;
                store.set((draft: any) => {
                  const st = draft.agents[agent];
                  if (st) {
                    st.activeSessionIds = (st.activeSessionIds || []).filter((id: string) => id !== sid);
                  }
                });
              }
              break;
            }

            case "error": {
              const errText = event.errorText;
              if (errText.includes("turn limit") || errText.includes("cycle detected")) {
                update(agent, { turnLimitMessage: errText });
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
        const effectiveSessionId = receivedSessionId ?? store.get().agents[agent]?.activeSessionId;
        if (!isError && !receivedFinishEvent && effectiveSessionId) {
          scheduleReconnect(agent, effectiveSessionId, reconnectAttempt);
          return;
        }

        if (!isError) {
          update(agent, {
            connectionPhase: "idle",
            connectionError: null,
            reconnectAttempt: 0,
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

    // Save and invalidate React Query caches, switch to history mode
    if (!signal.aborted) {
      if (receivedSessionId) {
        _onSessionId?.(agent, receivedSessionId);
      }
      queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
      const completedSessionId = receivedSessionId ?? store.get().agents[agent]?.activeSessionId;
      if (completedSessionId) {
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(completedSessionId) });
        // Switch to history mode so UI renders from DB rows via convertHistory —
        // identical to what the user sees after F5 reload.
        update(agent, { messageSource: { mode: "history", sessionId: completedSessionId } });
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
    abortLocalOnly,
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

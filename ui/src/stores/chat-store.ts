import { create } from "zustand";
import { devtools } from "zustand/middleware";
import { immer } from "zustand/middleware/immer";

import { apiGet, apiDelete, apiPatch, getToken } from "@/lib/api";
import { parseSSELines, parseSseEvent, parseContentParts } from "@/stores/sse-events";
import { IncrementalParser } from "@/lib/message-parser";
import type { SessionRow, MessageRow } from "@/types/api";
import { queryClient } from "@/lib/query-client";
import { qk } from "@/lib/queries";

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Generate UUID v4 — crypto.randomUUID in secure contexts, fallback for plain HTTP */
function uuid(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  // Fallback for non-secure contexts (HTTP, not HTTPS)
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
  });
}

// ── Constants ────────────────────────────────────────────────────────────────

const SESSIONS_PAGE_SIZE = 40;
const MESSAGES_HISTORY_LIMIT = 100;
export const MAX_INPUT_LENGTH = 32_000;
export const STREAM_THROTTLE_MS = 50;
// ── Message types (replaces AI SDK UIMessage dependency) ────────────────────

export interface TextPart {
  type: "text";
  text: string;
}

export interface ReasoningPart {
  type: "reasoning";
  text: string;
}

export interface FilePart {
  type: "file";
  url: string;
  mediaType: string;
}

export interface SourceUrlPart {
  type: "source-url";
  url: string;
  title?: string;
}

export type ToolPartState =
  | "input-streaming"
  | "input-available"
  | "output-available"
  | "output-error"
  | "output-denied";

export interface ToolPart {
  type: "tool";
  toolCallId: string;
  toolName: string;
  state: ToolPartState;
  input: Record<string, unknown>;
  output?: unknown;
  errorText?: string;
}

export interface RichCardPart {
  type: "rich-card";
  cardType: "table" | "metric" | "agent-turn";
  data: Record<string, unknown>;
}

export type MessagePart =
  | TextPart
  | ReasoningPart
  | FilePart
  | SourceUrlPart
  | ToolPart
  | RichCardPart;

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  parts: MessagePart[];
  createdAt?: string;
  /** Per-message agent identity (for multi-agent sessions). */
  agentId?: string;
  /** Optimistic send status (SSE-03). Undefined means confirmed (from history/sync). */
  status?: "sending" | "confirmed" | "failed";
}

// ── Connection phase FSM (FSM-01) ────────────────────────────────────────────

/**
 * Single authoritative phase enum for stream lifecycle state.
 * FSM-01: authoritative connection phase enum.
 * "complete" is a transient phase between finish event and finalizeStream.
 * "reconnecting" is set when stream drops mid-run and backoff retry is pending.
 */
export type ConnectionPhase = "idle" | "submitted" | "streaming" | "reconnecting" | "complete" | "error";

export function isActivePhase(phase: ConnectionPhase | undefined): boolean {
  return phase === "submitted" || phase === "streaming" || phase === "reconnecting";
}

// ── MessageSource discriminated union (HIST-02) ─────────────────────────────

/**
 * Discriminated union for message source mode.
 * Replaces the dual-semantics of viewMode + liveMessages fields.
 * - "new-chat": no session selected, no messages
 * - "live": active or recently completed stream, messages held in store
 * - "history": viewing a DB session snapshot, messages fetched via React Query
 */
export type MessageSource =
  | { mode: "new-chat" }
  | { mode: "live"; messages: ChatMessage[] }
  | { mode: "history"; sessionId: string };

/** Helper: extract live messages from a MessageSource union. */
function getLiveMessages(source: MessageSource): ChatMessage[] {
  return source.mode === "live" ? source.messages : [];
}

// ── Per-agent state ─────────────────────────────────────────────────────────

interface AgentState {
  activeSessionId: string | null;
  /** Discriminated union replacing the old liveMessages + viewMode duality. */
  messageSource: MessageSource;
  streamError: string | null;
  /** FSM-01: authoritative connection phase enum. */
  connectionPhase: ConnectionPhase;
  connectionError: string | null;
  /** When true, next sendMessage will force backend to create a new session. */
  forceNewSession: boolean;
  /** Server-driven list of session IDs currently being processed.
   *  Updated ONLY from WS agent_processing events — never optimistically.
   *  Array (not Set) because Immer doesn't support Set without enableMapSet(). */
  activeSessionIds: string[];
  /** How many messages to show at once (user can load more). */
  renderLimit: number;
  /** Per-session model override (null = use agent default). */
  modelOverride: string | null;
  /** Agent that will respond to the current message (from @-mention parsing). */
  pendingTargetAgent: string | null;
  /** Ordered list of agent names per assistant message turn (for multi-agent identity). */
  agentTurns: string[];
  /** Turn counter for multi-agent turn loop (incremented on each agent-turn event). */
  turnCount: number;
  /** Inline message when turn limit or cycle detection stops the loop. */
  turnLimitMessage: string | null;
  /** Per-agent stream generation counter (CLN-02 HIST-03) — detects stale SSE deltas. */
  streamGeneration: number;
}

function emptyAgentState(): AgentState {
  return {
    activeSessionId: null,
    messageSource: { mode: "new-chat" },
    streamError: null,
    connectionPhase: "idle",
    connectionError: null,
    forceNewSession: false,
    activeSessionIds: [],
    renderLimit: 100,
    modelOverride: null,
    pendingTargetAgent: null,
    agentTurns: [],
    turnCount: 0,
    turnLimitMessage: null,
    streamGeneration: 0,
  };
}

// ── Store interface ─────────────────────────────────────────────────────────

const LAST_SESSION_KEY = "hydeclaw.chat.lastSession";

// ── CLN-02: Encapsulated non-serializable state ──────────────────────────────
// AbortController and setTimeout handles are not plain objects — Immer cannot
// proxy or freeze them. They live in private Maps behind accessor helpers so
// no bare module-scope mutable Records remain.

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

// ── Reconnect constants (SSE-02) ─────────────────────────────────────────────
const MAX_RECONNECT_ATTEMPTS = 3;
const RECONNECT_DELAY_BASE_MS = 1000;

interface ChatStore {
  /** Per-agent state map. */
  agents: Record<string, AgentState>;
  /** Currently selected agent name. */
  currentAgent: string;
  /** Cache: sessionId -> participant list (updated from API responses and WS events). */
  sessionParticipants: Record<string, string[]>;

  // ── Actions ──
  setCurrentAgent: (name: string) => void;
  updateSessionParticipants: (sessionId: string, participants: string[]) => void;
  selectSession: (sessionId: string, forAgent?: string) => Promise<void>;
  /** Select a session by ID for a specific agent. */
  selectSessionById: (agent: string, sessionId: string) => void;
  newChat: () => void;
  /** Silently refresh history messages without loading indicator (used by WS session_updated). */
  refreshHistory: (sessionId: string, agentName?: string) => void;
  clearError: () => void;

  sendMessage: (text: string) => void;
  stopStream: () => void;
  regenerate: () => void;
  regenerateFrom: (messageId: string) => void;

  resumeStream: (agent: string, sessionId: string) => void;
  setThinking: (agent: string, sessionId: string | null) => void;
  setThinkingLevel: (level: number) => void;
  /** Server-driven: mark a session as actively processing. */
  markSessionActive: (agent: string, sessionId: string) => void;
  /** Server-driven: mark a session as no longer processing. */
  markSessionInactive: (agent: string, sessionId: string) => void;
  setModelOverride: (agent: string, model: string | null) => Promise<void>;
  renameSession: (sessionId: string, title: string) => Promise<void>;
  deleteSession: (sessionId: string) => Promise<void>;
  deleteAllSessions: () => Promise<void>;
  deleteMessage: (messageId: string) => Promise<void>;
  loadEarlierMessages: (agent: string) => void;
  exportSession: () => Promise<void>;

  // ── Internal ──
  _selectCounter: Record<string, number>;
}

// ── localStorage helpers ────────────────────────────────────────────────────

export function saveLastSession(agent: string, sessionId?: string) {
  try {
    const data = loadLastSession();
    data.agent = agent;
    if (sessionId) data.sessions = { ...data.sessions, [agent]: sessionId };
    localStorage.setItem(LAST_SESSION_KEY, JSON.stringify(data));
  } catch { /* ignore */ }
}

function clearLastSessionId(agent: string) {
  try {
    const data = loadLastSession();
    if (data.sessions?.[agent]) {
      delete data.sessions[agent];
      localStorage.setItem(LAST_SESSION_KEY, JSON.stringify(data));
    }
  } catch { /* ignore */ }
}

function loadLastSession(): { agent?: string; sessions?: Record<string, string>; sessionId?: string } {
  try {
    const saved = localStorage.getItem(LAST_SESSION_KEY);
    if (saved) return JSON.parse(saved);
  } catch { /* ignore */ }
  return {};
}


// ── History conversion (MessageRow[] → ChatMessage[]) ───────────────────────

/**
 * Converts flat database rows into structured ChatMessage objects.
 * Implements "Virtual Merging" (Stage 2): consecutive assistant/tool blocks
 * from the same agent are merged into a single visual message to ensure
 * stable tool grouping and consistent identity.
 */
export function convertHistory(rows: MessageRow[], isAgentStreaming?: boolean): ChatMessage[] {
  // Filter out streaming placeholder messages ONLY if we have an active live stream
  // that will provide the same content. If not, show them as fallback (history).
  const filtered = rows.filter(m => {
    if (m.status === "streaming" && isAgentStreaming) return false;
    return true;
  });

  const messages: ChatMessage[] = [];
  let lastAssistantMsg: ChatMessage | null = null;
  let lastAgentId: string | undefined = undefined;

  // Tool call map for resolving tool names/inputs from the main assistant record
  const toolCallMap = new Map<string, { name: string; arguments: unknown }>();
  for (const m of filtered) {
    if (m.role === "assistant" && m.tool_calls) {
      const calls = m.tool_calls as Array<{ id: string; name: string; arguments?: unknown }>;
      if (Array.isArray(calls)) {
        for (const tc of calls) {
          if (tc.id) toolCallMap.set(tc.id, { name: tc.name || "tool", arguments: tc.arguments ?? {} });
        }
      }
    }
  }

  for (const m of filtered) {
    if (m.role === "user") {
      // Finalize any pending assistant message before starting a user block
      if (lastAssistantMsg) {
        messages.push(lastAssistantMsg);
        lastAssistantMsg = null;
      }
      if (m.agent_id) lastAgentId = m.agent_id;
      messages.push({
        id: m.id,
        role: "user",
        parts: [{ type: "text", text: m.content || "" }],
        createdAt: m.created_at,
        agentId: m.agent_id ?? undefined,
      });
    } else if (m.role === "assistant" && !m.tool_call_id) {
      // Assistant text block
      const assistantAgentId = m.agent_id ?? lastAgentId;
      if (m.agent_id) lastAgentId = m.agent_id;

      const newParts = parseContentParts(m.content || "");

      // D-01: No merging. Each assistant DB row becomes its own ChatMessage.
      // Virtual Merging was removed because it breaks tool call ordering —
      // tools must appear between the assistant messages that invoked them.
      if (lastAssistantMsg) messages.push(lastAssistantMsg);
      lastAssistantMsg = {
        id: m.id,
        role: "assistant",
        parts: newParts,
        createdAt: m.created_at,
        agentId: assistantAgentId,
      };
    } else if (m.role === "tool" && m.tool_call_id) {
      // Tool result block — always attach to the latest assistant message
      if (lastAssistantMsg) {
        const tc = toolCallMap.get(m.tool_call_id);
        
        // Extract inline files (__file__: markers)
        const lines = (m.content || "").split("\n");
        const cleanLines: string[] = [];
        for (const line of lines) {
          if (line.startsWith("__file__:")) {
            try {
              const meta = JSON.parse(line.slice("__file__:".length));
              if (meta.url) {
                lastAssistantMsg.parts.push({
                  type: "file",
                  url: meta.url,
                  mediaType: meta.mediaType || "image/png",
                });
              }
            } catch { /* ignore */ }
          } else {
            cleanLines.push(line);
          }
        }

        lastAssistantMsg.parts.push({
          type: "tool",
          toolCallId: m.tool_call_id,
          toolName: tc?.name || "tool",
          state: "output-available",
          input: (tc?.arguments as Record<string, unknown>) ?? {},
          output: cleanLines.join("\n"),
        });
      }
    }
  }
  
  if (lastAssistantMsg) messages.push(lastAssistantMsg);

  // Final pass: filter empty messages and stabilize referential identity
  return messages.filter(m => m.parts.length > 0);
}

/**
 * Read-through cache peek — called from Zustand store actions where React hooks
 * are unavailable. Components access this data via useSessionMessages() hook.
 * See ARCH-02 audit (phase 34): queryClient.getQueryData is intentional here and
 * in sendMessage(); no React component calls getQueryData directly.
 */
function getCachedHistoryMessages(sessionId: string | null): ChatMessage[] {
  if (!sessionId) return [];
  const cached = queryClient.getQueryData<{ messages: MessageRow[] }>(qk.sessionMessages(sessionId));
  return cached ? convertHistory(cached.messages) : [];
}

// ── Store implementation ────────────────────────────────────────────────────

export const useChatStore = create<ChatStore>()(
  devtools(
    immer((set, get) => {
  // ── Internal: ensure agent state exists ──
  function ensure(agent: string): AgentState {
    const s = get().agents[agent];
    if (s) return s;
    const fresh = emptyAgentState();
    set((draft) => { draft.agents[agent] = fresh; });
    return fresh;
  }

  function update(agent: string, patch: Partial<AgentState>) {
    set((draft) => {
      if (!draft.agents[agent]) draft.agents[agent] = emptyAgentState();
      Object.assign(draft.agents[agent], patch);
    });
  }

  // ── Debounced UI state persistence to server ──
  const uiStateSaveTimers: Record<string, ReturnType<typeof setTimeout>> = {};
  function saveUiState(agent: string) {
    clearTimeout(uiStateSaveTimers[agent]);
    uiStateSaveTimers[agent] = setTimeout(() => {
      const st = get().agents[agent];
      if (!st?.activeSessionId) return;
      apiPatch(`/api/sessions/${st.activeSessionId}`, {
        ui_state: { connectionPhase: st.connectionPhase },
      }).catch((e) => { console.warn("[chat] save failed:", e); });
    }, 500);
  }

  // ── Guaranteed UI state flush on tab close ──────────────────────────────
  // [MOVED TO REACT EFFECT IN ChatThread.tsx to prevent listener leaks]

  /**
   * Resume an active backend stream after page reload.
   * Connects to GET /api/chat/{sessionId}/stream and processes replay + live events.
   */
  function resumeStream(agent: string, sessionId: string, reconnectAttempt = 0) {
    // Don't resume if already streaming (but allow reconnect path even in "reconnecting" phase)
    const st = get().agents[agent];
    if (st && st.connectionPhase === "streaming") return;

    // Clear any existing reconnect timer before starting a new stream
    const existingTimer = getReconnectTimer(agent);
    if (existingTimer) {
      clearTimeout(existingTimer);
      setReconnectTimer(agent, null);
    }
    abortActiveStream(agent);
    update(agent, { streamGeneration: (get().agents[agent]?.streamGeneration ?? 0) + 1 });
    const myGeneration = get().agents[agent]?.streamGeneration ?? 1;
    const controller = new AbortController();
    setAbortCtrl(agent, controller);

    // Seed with history from React Query cache so UI shows messages immediately (Fix A).
    // Read getCachedHistoryMessages BEFORE the update() call — never call get() inside set().
    // Use the passed sessionId parameter directly: activeSessionId may not be set yet in store
    // (e.g. first render after F5 before WS delivers session state).
    const existingSt = get().agents[agent];
    const seedMessages = existingSt?.messageSource.mode === "live"
      ? existingSt.messageSource.messages
      : getCachedHistoryMessages(sessionId);

    update(agent, {
      streamError: null,
      connectionPhase: "streaming",
      connectionError: null,
      messageSource: { mode: "live", messages: seedMessages },
    });

    const token = getToken();

    fetch(`/api/chat/${sessionId}/stream`, {
      method: "GET",
      headers: { Authorization: `Bearer ${token}` },
      signal: controller.signal,
    })
      .then((resp) => {
        if (resp.status === 204) {
          // No active stream — engine already finished.
          // Transition to history mode so useSessionMessages fetches fresh data (Fix B).
          update(agent, { connectionPhase: "idle", messageSource: { mode: "history", sessionId } });
          return;
        }
        if (!resp.ok) {
          return resp.text().then((t) => { throw new Error(t || `HTTP ${resp.status}`); });
        }
        return processSSEStream(agent, resp.body!, controller.signal, myGeneration, sessionId, reconnectAttempt);
      })
      .catch((err) => {
        if (err.name === "AbortError") return;
        // Network error during reconnect — schedule next retry
        if (reconnectAttempt < MAX_RECONNECT_ATTEMPTS) {
          scheduleReconnect(agent, sessionId, reconnectAttempt);
        } else {
          update(agent, { connectionPhase: "idle" });
        }
      });
  }

  /** Abort active stream for an agent and reset status. */
  function abortActiveStream(agent: string) {
    // Clear any pending reconnect timer first — prevents reconnect after abort
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

  // ── Reconnect scheduling (SSE-02) ────────────────────────────────────────────
  /**
   * Schedule an exponential-backoff reconnect for an agent.
   * Called when processSSEStream exits without receiving a finish event.
   * Cleared by abortActiveStream/stopStream (user-initiated aborts must NOT retry).
   */
  function scheduleReconnect(agent: string, sessionId: string, attempt: number) {
    if (attempt >= MAX_RECONNECT_ATTEMPTS) {
      const sid = sessionId ?? get().agents[agent]?.activeSessionId;
      update(agent, {
        streamError: "Connection lost after retries",
        connectionPhase: "error",
        connectionError: "Connection lost after retries",
        // Fall back to history mode so stale live messages don't stick
        messageSource: sid ? { mode: "history", sessionId: sid } : { mode: "new-chat" },
      });
      return;
    }
    update(agent, { connectionPhase: "reconnecting", connectionError: null });
    const delay = RECONNECT_DELAY_BASE_MS * Math.pow(2, attempt);
    setReconnectTimer(agent, setTimeout(() => {
      setReconnectTimer(agent, null);
      // resumeStream handles 204 (engine finished) gracefully
      resumeStream(agent, sessionId, attempt + 1);
    }, delay));
  }

  // ── SSE stream handler ──
  function startStream(agent: string, sessionId: string | null, messages: ChatMessage[], userText: string) {
    abortActiveStream(agent);
    update(agent, { streamGeneration: (get().agents[agent]?.streamGeneration ?? 0) + 1 });
    const myGeneration = get().agents[agent]?.streamGeneration ?? 1;
    const controller = new AbortController();
    setAbortCtrl(agent, controller);

    // Build user message — optimistic status: "sending" until data-session-id confirms receipt
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

    // Build request body — backend only uses the last user message + session_id
    const agentState = get().agents[agent];
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

    const token = getToken();

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
        set((draft) => {
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
    // Fall back to primary agent name so single-agent sessions never produce undefined agentId.
    let currentRespondingAgent: string | null = get().agents[agent]?.pendingTargetAgent ?? agent;

    function flushText() {
      // Snapshot accumulated text/reasoning into parts array at current position.
      // This preserves text-tool-text ordering during streaming.
      const flushed = incrementalParser.flush();
      if (flushed.length > 0) {
        parts.push(...flushed);
      }
    }

    function pushUpdate() {
      // Guard: stale stream — a newer stream has started, discard updates
      if (generation !== (get().agents[agent]?.streamGeneration ?? 0)) return;
      // Guard: don't update store after abort (prevents race with stopStream)
      if (signal.aborted) return;

      // Get trailing text that hasn't been flushed yet (current typing)
      const trailingContent = incrementalParser.processDelta("");

      set((draft) => {
        const st = draft.agents[agent];
        if (!st) return;
        // Ensure messageSource is in live mode (it should be from startStream)
        if (st.messageSource.mode !== "live") {
          st.messageSource = { mode: "live", messages: [] };
        }
        const liveMessages = st.messageSource.messages;
        const existing = liveMessages.findIndex((m: ChatMessage) => m.id === assistantId);

        // Parts array has flushed text + tools in correct order.
        // Append trailing (unflushed) text at the end.
        const allParts = [...parts, ...trailingContent];

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

    // Throttle UI updates to ~20fps (50ms) — reduces renders from 60/sec to 20/sec.
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
              if (sid && generation === (get().agents[agent]?.streamGeneration ?? 0)) {
                receivedSessionId = sid;
                // SSE-03: Confirm the optimistic user message — server has accepted it.
                // Find the last "sending" user message and mark it confirmed.
                set((draft) => {
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
                saveLastSession(agent, sid);
                // Session status is now server-driven via WS agent_processing events.
                // No optimistic updates needed — WS event arrives ~simultaneously.

                // Populate sessionParticipants cache from React Query session data
                const sessionsData = queryClient.getQueryData<{ sessions: SessionRow[] }>(
                  qk.sessions(agent)
                );
                const session = sessionsData?.sessions.find(s => s.id === sid);
                if (session?.participants) {
                  get().updateSessionParticipants(sid, session.participants);
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
              // Dedup: remove resume placeholder (id starts with "resume-") and any
              // seeded message with same ID to prevent duplicates on stream resume
              const stNow = get().agents[agent];
              if (stNow && stNow.messageSource.mode === "live") {
                const currentMessages = stNow.messageSource.messages;
                const deduped = currentMessages.filter(
                  (m) => m.id !== newId && !m.id.startsWith("resume-")
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
              // Incremental parser accumulates state across text blocks
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
              // Chunks were only needed for streaming display; actual input is now available — free memory
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
                // Agent turn separator: update tracking state but DON'T push to parts
                // or scheduleUpdate — this is a control event, not message content.
                // Pushing would create a phantom assistant message after finish reset.
                currentRespondingAgent = event.data.agentName as string;
                const currentTurnCount = get().agents[agent]?.turnCount ?? 0;
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
              // Stage 3: Differential Sync. Instead of replacing all liveMessages,
              // we only update the assistant message if it matches our current assistantId.
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

              set((draft) => {
                const st = draft.agents[agent];
                if (!st) return;
                // Ensure we're in live mode for sync updates
                if (st.messageSource.mode !== "live") {
                  st.messageSource = { mode: "live", messages: [] };
                }
                const liveMessages = st.messageSource.messages;
                const existingIdx = liveMessages.findIndex(m => m.id === assistantId);
                if (existingIdx >= 0) {
                  // Differential update: preserves user messages and other assistant messages
                  liveMessages[existingIdx].parts = syncParts;
                } else {
                  // Fallback: if not found, it might be a clean resume, so we seed
                  const userMsgs = liveMessages.filter(m => m.role === "user");
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
                const inTurnLoop = !!get().agents[agent]?.pendingTargetAgent;
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
              // Mark natural end — distinguishes from connection drop in finally block
              receivedFinishEvent = true;
              // Cancel any pending update and do final synchronous update
              cancelScheduledUpdate();
              flushText(); // Snapshot remaining text into parts at correct position
              pushUpdate(); // Final render with all parts in correct order
              // FSM-04: Reset incremental parser state so next agent turn starts clean.
              // Prevents reasoning state from leaking from one agent's output to the next.
              incrementalParser.reset();
              // CRITICAL for multi-agent turn loop: reset state for next agent turn.
              // Without this, events between finish and next start (e.g. agent-turn rich card)
              // would overwrite the finalized message with wrong agentId.
              assistantId = uuid();
              assistantCreatedAt = new Date().toISOString();
              parts = [];
              break;
            }

            case "error": {
              const errText = event.errorText;
              if (errText.includes("turn limit") || errText.includes("cycle detected")) {
                // Turn management message — show inline as info card, not as error banner
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
      // Execute any pending update synchronously instead of cancelling it
      // (prevents losing the final text-delta that was scheduled but not yet rendered)
      if (updateScheduled) {
        cancelScheduledUpdate();
        pushUpdate(); // Execute the update that setTimeout+rAF would have done
      }
      // Always flush remaining text (including on abort — preserves partial response)
      flushText();
      if (!signal.aborted) {
        // Only push if there's content — avoids phantom empty message after finish reset
        if (parts.length > 0) pushUpdate();

        // SSE-02: Detect connection drop (stream ended without finish event).
        // If we have a session ID and haven't been aborted, schedule reconnect.
        const isError = get().agents[agent]?.connectionPhase === "error";
        if (!isError && !receivedFinishEvent && receivedSessionId) {
          // Connection dropped mid-stream — schedule exponential backoff reconnect
          scheduleReconnect(agent, receivedSessionId, reconnectAttempt);
          return;
        }

        // Preserve error status if error event was already received
        if (!isError) {
          const completedSessionId = receivedSessionId ?? get().agents[agent]?.activeSessionId;
          update(agent, {
            connectionPhase: "idle",
            connectionError: null,
            pendingTargetAgent: null,
            turnCount: 0,
            // Transition to history mode so showThinking doesn't trigger on stale "live" mode
            messageSource: completedSessionId
              ? { mode: "history", sessionId: completedSessionId }
              : { mode: "new-chat" },
          });
        }
        saveUiState(agent);
        // Session status is server-driven via WS agent_processing events — no optimistic update needed.
      } else if (parts.length > 0) {
        // On abort: save partial response to live messages but keep idle status
        // (stopStream already set connectionPhase to "idle")
        const st = get().agents[agent];
        if (st) {
          const assistantMsg: ChatMessage = {
            id: assistantId,
            role: "assistant",
            parts: [...parts],
            createdAt: assistantCreatedAt,
            agentId: currentRespondingAgent ?? undefined,
          };
          const currentMessages = getLiveMessages(st.messageSource);
          const existing = currentMessages.findIndex((m) => m.id === assistantId);
          const updated =
            existing >= 0
              ? currentMessages.map((m, i) => (i === existing ? assistantMsg : m))
              : [...currentMessages, assistantMsg];
          update(agent, { messageSource: { mode: "live", messages: updated } });
        }
      }
    }

    // Save and invalidate React Query caches (skip on abort — stream was cancelled intentionally)
    if (!signal.aborted) {
      if (receivedSessionId) {
        saveLastSession(agent, receivedSessionId);
      }
      // Refresh session list and session messages in React Query cache
      queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
      const completedSessionId = receivedSessionId ?? get().agents[agent]?.activeSessionId;
      if (completedSessionId) {
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(completedSessionId) });
      }
    }
  }

  return {
    agents: {},
    currentAgent: "",
    sessionParticipants: {},
    _selectCounter: {},

    updateSessionParticipants: (sessionId: string, participants: string[]) => {
      set((draft) => {
        draft.sessionParticipants[sessionId] = participants;
      });
    },

    setCurrentAgent: (name: string) => {
      const prev = get().currentAgent;
      if (prev === name) return;

      // Page-load initialization (prev is empty) — just set the agent,
      // DON'T wipe session state. The restore effect in page.tsx will handle it.
      if (!prev) {
        ensure(name);
        set({ currentAgent: name });
        queryClient.invalidateQueries({ queryKey: qk.sessions(name) });
        return;
      }

      // Check if current session is multi-agent and includes the new agent
      const prevState = get().agents[prev];
      const activeSessionId = prevState?.activeSessionId;

      if (activeSessionId) {
        const participants = get().sessionParticipants[activeSessionId];
        if (participants && participants.includes(name)) {
          ensure(name);
          update(name, {
            activeSessionId,
            messageSource: prevState?.messageSource ?? { mode: "new-chat" },
            connectionPhase: prevState?.connectionPhase ?? "idle",
          });
          set({ currentAgent: name });
          saveLastSession(name, activeSessionId);
          return;
        }
      }

      // User-initiated agent switch — abort stream, reset state
      const prevCtrl = getAbortCtrl(prev);
      if (prevCtrl) {
        prevCtrl.abort();
        setAbortCtrl(prev, null);
        update(prev, { connectionPhase: "idle" });
      }
      ensure(name);
      update(name, {
        activeSessionId: null,
        messageSource: { mode: "new-chat" },
        streamError: null,
        connectionPhase: "idle",
        connectionError: null,
        forceNewSession: true,
      });
      set({ currentAgent: name });
      clearLastSessionId(name);
      saveLastSession(name);
      queryClient.invalidateQueries({ queryKey: qk.sessions(name) });
    },

    selectSession: async (sessionId: string, forAgent?: string) => {
      const agent = forAgent ?? get().currentAgent;
      ensure(agent);

      // If re-selecting the same session that's currently streaming, just switch to live view
      const currentState = get().agents[agent];
      if (currentState?.activeSessionId === sessionId && isActivePhase(currentState.connectionPhase)) {
        // Already in live mode — no change needed (messageSource should already be live)
        return;
      }

      abortActiveStream(agent);

      update(agent, {
        activeSessionId: sessionId,
        messageSource: { mode: "history", sessionId },
        forceNewSession: false,
        renderLimit: 100,
      });
      saveLastSession(agent, sessionId);
      // Data fetching is handled by useSessionMessages() React Query hook in assistant-runtime.tsx
      // Polling for in-progress sessions is handled by refetchInterval in useSessionMessages
    },

    selectSessionById: (agent: string, sessionId: string) => {
      // Switch to the agent and select the session
      set({ currentAgent: agent });
      ensure(agent);
      // Abort any active stream for this agent
      const sbiCtrl = getAbortCtrl(agent);
      if (sbiCtrl) {
        sbiCtrl.abort();
        setAbortCtrl(agent, null);
      }
      update(agent, {
        activeSessionId: sessionId,
        messageSource: { mode: "history", sessionId },
        forceNewSession: false,
        connectionPhase: "idle",
      });
      saveLastSession(agent, sessionId);
      // Data fetching handled by useSessionMessages() React Query hook
    },

    newChat: () => {
      const agent = get().currentAgent;
      getAbortCtrl(agent)?.abort();
      setAbortCtrl(agent, null);
      update(agent, {
        activeSessionId: null,
        messageSource: { mode: "new-chat" },
        streamError: null,
        connectionPhase: "idle",
        connectionError: null,
        forceNewSession: true,
      });
      saveLastSession(agent);
    },

    refreshHistory: (sessionId: string, _agentName?: string) => {
      // Invalidate React Query cache — useSessionMessages will re-fetch
      queryClient.invalidateQueries({ queryKey: qk.sessionMessages(sessionId) });
    },

    clearError: () => {
      const agent = get().currentAgent;
      update(agent, { streamError: null });
    },

    setThinking: (agent: string, sessionId: string | null) => {
      const st = get().agents[agent];
      const updates: Partial<AgentState> = {};

      // On reload (before restore): Zustand activeSessionId is null — set it so
      // useSessionMessages can fetch and the DB streaming record is visible.
      // Guard: only when null AND not in "new chat" mode — don't override newChat().
      if (sessionId !== null && st?.activeSessionId == null && !st?.forceNewSession) {
        updates.activeSessionId = sessionId;
      }

      if (Object.keys(updates).length > 0) update(agent, updates);
    },

    setThinkingLevel: (level: number) => {
      const clampedLevel = Math.max(0, Math.min(5, level));
      get().sendMessage(`/think ${clampedLevel}`);
    },

    markSessionActive: (agent: string, sessionId: string) => {
      ensure(agent);
      set((draft) => {
        const st = draft.agents[agent];
        if (!st) return;
        if (!st.activeSessionIds.includes(sessionId)) {
          st.activeSessionIds.push(sessionId);
        }
      });
    },

    markSessionInactive: (agent: string, sessionId: string) => {
      ensure(agent);
      set((draft) => {
        const st = draft.agents[agent];
        if (!st) return;
        st.activeSessionIds = st.activeSessionIds.filter((id: string) => id !== sessionId);
      });
    },

    setModelOverride: async (agent, model) => {
      update(agent, { modelOverride: model });
      const token = getToken();
      await fetch(`/api/agents/${encodeURIComponent(agent)}/model-override`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ model }),
      }).catch((e) => { console.warn("[chat] save failed:", e); }); // fail silently — store already updated optimistically
    },

    resumeStream: (agent: string, sessionId: string) => resumeStream(agent, sessionId),

    sendMessage: (text: string) => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();

      if (isActivePhase(st.connectionPhase)) return;

      // Parse @-mention to set pendingTargetAgent for thinking indicator
      const mentionMatch = text.match(/^@(\S+)/);
      const targetAgent = mentionMatch ? mentionMatch[1] : null;
      update(agent, { pendingTargetAgent: targetAgent });

      let sessionId = st.activeSessionId;
      let seedMessages: ChatMessage[] = [];

      if (st.messageSource.mode === "history") {
        // Continue from history — get messages from React Query cache.
        // Do NOT flip messageSource here; startStream sets messageSource atomically.
        seedMessages = getCachedHistoryMessages(sessionId);
      } else if (st.messageSource.mode === "live" && st.messageSource.messages.length > 0) {
        seedMessages = st.messageSource.messages;
      }

      startStream(agent, sessionId, seedMessages, text);
    },

    stopStream: () => {
      const agent = get().currentAgent;
      // Clear any pending reconnect timer — user abort must not trigger reconnect
      const stopTimer = getReconnectTimer(agent);
      if (stopTimer) {
        clearTimeout(stopTimer);
        setReconnectTimer(agent, null);
      }
      getAbortCtrl(agent)?.abort();
      setAbortCtrl(agent, null);
      update(agent, { connectionPhase: "idle" });
    },

    regenerate: () => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();

      // Abort any active stream first
      if (isActivePhase(st.connectionPhase)) {
        getAbortCtrl(agent)?.abort();
        setAbortCtrl(agent, null);
        update(agent, { connectionPhase: "idle" });
      }

      let sessionId = st.activeSessionId;
      let messages: ChatMessage[];

      if (st.messageSource.mode === "history") {
        // Do NOT flip messageSource here; startStream sets messageSource atomically.
        messages = getCachedHistoryMessages(sessionId);
      } else {
        messages = getLiveMessages(st.messageSource);
      }

      // Remove last assistant message
      if (messages.length > 0 && messages[messages.length - 1].role === "assistant") {
        messages = messages.slice(0, -1);
      }

      // Get last user message text
      const lastUser = [...messages].reverse().find((m) => m.role === "user");
      if (!lastUser) return;
      const userText = lastUser.parts
        .filter((p): p is TextPart => p.type === "text")
        .map((p) => p.text)
        .join("\n");

      // Remove last user message too (startStream will re-add it)
      messages = messages.slice(0, messages.lastIndexOf(lastUser));

      startStream(agent, sessionId, messages, userText);
    },

    regenerateFrom: (messageId: string) => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();

      if (isActivePhase(st.connectionPhase)) {
        getAbortCtrl(agent)?.abort();
        setAbortCtrl(agent, null);
        update(agent, { connectionPhase: "idle" });
      }

      let sessionId = st.activeSessionId;
      let messages: ChatMessage[];

      if (st.messageSource.mode === "history") {
        // Do NOT flip messageSource here; startStream sets messageSource atomically.
        messages = getCachedHistoryMessages(sessionId);
      } else {
        messages = getLiveMessages(st.messageSource);
      }

      // Find the target user message and truncate everything after it
      const targetIdx = messages.findIndex((m) => m.id === messageId);
      if (targetIdx === -1) {
        // Fallback to normal regenerate if message not found
        get().regenerate();
        return;
      }

      const targetMsg = messages[targetIdx];
      if (targetMsg.role !== "user") {
        get().regenerate();
        return;
      }

      const userText = targetMsg.parts
        .filter((p) => p.type === "text")
        .map((p) => (p as { text: string }).text)
        .join("\n");

      // Keep only messages before the target (startStream re-adds the user message)
      const seedMessages = messages.slice(0, targetIdx);

      startStream(agent, sessionId, seedMessages, userText);
    },

    renameSession: async (sessionId: string, title: string) => {
      const agent = get().currentAgent;
      await apiPatch(`/api/sessions/${sessionId}`, { title });
      queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
    },

    deleteSession: async (sessionId: string) => {
      const agent = get().currentAgent;
      await apiDelete(`/api/sessions/${sessionId}?agent=${encodeURIComponent(agent)}`);
      const st = get().agents[agent];
      if (st?.activeSessionId === sessionId) {
        // Use captured `agent` — currentAgent may have changed during await
        getAbortCtrl(agent)?.abort();
        setAbortCtrl(agent, null);
        update(agent, {
          activeSessionId: null, messageSource: { mode: "new-chat" },
          streamError: null,
          connectionPhase: "idle", connectionError: null,
          forceNewSession: true,
        });
        saveLastSession(agent);
      }
      queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
    },

    deleteAllSessions: async () => {
      const agent = get().currentAgent;
      await apiDelete(`/api/sessions?agent=${encodeURIComponent(agent)}`);
      // Use captured `agent` — currentAgent may have changed during await
      getAbortCtrl(agent)?.abort();
      setAbortCtrl(agent, null);
      update(agent, {
        activeSessionId: null, messageSource: { mode: "new-chat" },
        streamError: null,
        connectionPhase: "idle", connectionError: null,
        forceNewSession: true,
      });
      saveLastSession(agent);
      queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
    },

    loadEarlierMessages: (agent: string) => {
      set((draft) => {
        const st = draft.agents[agent];
        if (st) st.renderLimit = (st.renderLimit ?? 100) + 100;
      });
    },

    deleteMessage: async (messageId: string) => {
      const agent = get().currentAgent;
      await apiDelete(`/api/messages/${messageId}`);
      const st = get().agents[agent];
      if (!st) return;
      if (st.messageSource.mode === "history" && st.activeSessionId) {
        // Invalidate React Query cache to reload history
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(st.activeSessionId) });
      } else {
        const currentMessages = getLiveMessages(st.messageSource);
        update(agent, {
          messageSource: { mode: "live", messages: currentMessages.filter((m) => m.id !== messageId) },
        });
      }
    },

    exportSession: async () => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent];
      if (!st) return;

      const messages = st.messageSource.mode === "live"
        ? st.messageSource.messages
        : getCachedHistoryMessages(st.activeSessionId);
      if (messages.length === 0) return;

      const session = {
        id: st.activeSessionId ?? "unknown",
        agent_id: agent,
        user_id: "",
        channel: "web",
        started_at: messages[0]?.createdAt ?? new Date().toISOString(),
        last_message_at: new Date().toISOString(),
      };

      const { sessionToMarkdown } = await import("@/lib/format");
      const markdown = sessionToMarkdown(messages, session as import("@/types/api").SessionRow, agent);

      const blob = new Blob([markdown], { type: "text/markdown;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      try {
        const a = document.createElement("a");
        a.href = url;
        a.download = `${agent}-${new Date().toISOString().slice(0, 10)}.md`;
        document.body.appendChild(a);
        a.click();
        document.body.removeChild(a);
      } finally {
        URL.revokeObjectURL(url);
      }
    },
  };
    }),
    { name: "ChatStore", enabled: process.env.NODE_ENV !== "production" },
  ),
);

// ── Auto-restore helper (call once on mount) ───────────────────────────────

export function getInitialAgent(agents: string[]): string {
  const { agent: savedAgent } = loadLastSession();
  if (savedAgent && agents.includes(savedAgent)) return savedAgent;
  return agents[0] || "";
}

export function getLastSessionId(agent?: string): string | undefined {
  const data = loadLastSession();
  // Per-agent session lookup, fallback to legacy global sessionId
  if (agent && data.sessions?.[agent]) return data.sessions[agent];
  return data.sessionId;
}

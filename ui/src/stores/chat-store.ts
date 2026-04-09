import { create } from "zustand";
import { devtools } from "zustand/middleware";
import { immer } from "zustand/middleware/immer";

import { apiGet, apiPost, apiDelete, apiPatch, getToken } from "@/lib/api";
import { parseSSELines, parseSseEvent, parseContentParts, extractSseEventId } from "@/stores/sse-events";
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
  cardType: string;
  data: Record<string, unknown>;
}

export interface ContinuationSeparatorPart {
  type: "continuation-separator";
}

export interface StepGroupPart {
  type: "step-group";
  stepId: string;
  toolParts: ToolPart[];
  finishReason?: string;
  /** True while step is still receiving events */
  isStreaming: boolean;
}

export interface ApprovalPart {
  type: "approval";
  approvalId: string;
  toolName: string;
  toolInput: Record<string, unknown>;
  timeoutMs: number;
  receivedAt: number;
  status: "pending" | "approved" | "rejected" | "timeout_rejected";
  modifiedInput?: Record<string, unknown>;
}

export type MessagePart =
  | TextPart
  | ReasoningPart
  | FilePart
  | SourceUrlPart
  | ToolPart
  | RichCardPart
  | ContinuationSeparatorPart
  | StepGroupPart
  | ApprovalPart;

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  parts: MessagePart[];
  createdAt?: string;
  /** Per-message agent identity (for multi-agent sessions). */
  agentId?: string;
  /** Optimistic send status (SSE-03). Undefined means confirmed (from history/sync). */
  status?: "sending" | "confirmed" | "failed";
  /** Parent message ID in the tree (null for root/trunk messages). */
  parentMessageId?: string;
  /** The message this branch was forked from (set on fork-created user messages). */
  branchFromMessageId?: string;
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

// ── OPTI-03: Content hash reconciliation ────────────────────────────────────

/**
 * Fast djb2-style hash of a ChatMessage array.
 * Compares id + role + text content of parts — intentionally ignores createdAt
 * (timestamps differ between live SSE messages and DB history rows).
 * Used for render-optimization, not security.
 */
export function contentHash(messages: ChatMessage[]): string {
  let hash = 0;
  for (const m of messages) {
    const str =
      m.id +
      m.role +
      m.parts
        .map((p) =>
          p.type === "text"
            ? p.text
            : p.type + ("toolCallId" in p ? (p as { toolCallId: string }).toolCallId : ""),
        )
        .join("|");
    for (let i = 0; i < str.length; i++) {
      hash = ((hash << 5) - hash + str.charCodeAt(i)) | 0;
    }
  }
  return hash.toString(36);
}

/**
 * OPTI-03: Compare live messages with freshly-fetched history.
 * Returns null if content is identical (skip re-render), or returns history
 * messages when they differ (history has extra data or server post-processing).
 */
export function reconcileLiveWithHistory(
  live: ChatMessage[],
  history: ChatMessage[],
): ChatMessage[] | null {
  if (live.length === history.length && contentHash(live) === contentHash(history)) {
    return null; // identical — skip re-render
  }
  return history;
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
<<<<<<< HEAD
  /** NET-02: Current reconnect attempt count (0 when not reconnecting). */
  reconnectAttempt: number;
  /** NET-02: Max reconnect attempts (exposed for UI indicator). */
  maxReconnectAttempts: number;
  /** Branch selection state: parentMessageId -> selectedChildId. */
  selectedBranches: Record<string, string>;
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
    reconnectAttempt: 0,
    maxReconnectAttempts: MAX_RECONNECT_ATTEMPTS,
    selectedBranches: {},
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

// ── NET-02: Last event ID tracking for SSE resume ──────────────────────────
const _agentLastEventIds = new Map<string, string>();
function getLastEventId(agent: string): string | null { return _agentLastEventIds.get(agent) ?? null; }
function setLastEventId(agent: string, id: string) { _agentLastEventIds.set(agent, id); }
function clearLastEventId(agent: string) { _agentLastEventIds.delete(agent); }

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
  /** Switch branch at a fork point (client-side only, no server roundtrip). */
  switchBranch: (parentMessageId: string, selectedChildId: string) => void;
  /** Fork a user message and start a new stream with the edited content. */
  forkAndRegenerate: (messageId: string, newContent: string) => void;

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


// ── Tree-aware path resolution (BRNC-03) ────────────────────────────────────

/**
 * Given all messages (including all branches) and the user's branch selections,
 * returns the linear path of messages to display.
 *
 * Algorithm:
 * 1. Find root messages (parent_message_id === null). For trunk sessions, all
 *    messages are roots -- fall back to created_at order.
 * 2. Build a children map: parentId -> children sorted by created_at.
 * 3. Walk from root: at each node, if multiple children exist, pick the selected
 *    one (from selectedBranches) or default to the latest.
 * 4. Continue until no more children.
 */
export function resolveActivePath(
  rows: MessageRow[],
  selectedBranches: Record<string, string>,
): MessageRow[] {
  // If no rows have parent_message_id set, this is a trunk session -- return rows sorted by created_at
  const hasBranching = rows.some(r => r.parent_message_id !== null);
  if (!hasBranching) {
    return [...rows].sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
  }

  // Build children map
  const childrenOf = new Map<string, MessageRow[]>();
  const roots: MessageRow[] = [];

  for (const r of rows) {
    if (r.parent_message_id === null) {
      roots.push(r);
    } else {
      const siblings = childrenOf.get(r.parent_message_id) ?? [];
      siblings.push(r);
      childrenOf.set(r.parent_message_id, siblings);
    }
  }

  // Sort children by created_at within each group
  for (const [, children] of childrenOf) {
    children.sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
  }

  // There should be exactly one root for a well-formed session; pick first by created_at
  roots.sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
  if (roots.length === 0) return [];

  const path: MessageRow[] = [];
  let current: MessageRow | undefined = roots[0];

  while (current) {
    path.push(current);
    const children = childrenOf.get(current.id);
    if (!children || children.length === 0) break;

    // Pick selected child or default to latest
    const selectedId: string | undefined = selectedBranches[current.id];
    current = selectedId
      ? children.find(c => c.id === selectedId) ?? children[children.length - 1]
      : children[children.length - 1];
  }

  return path;
}

/** Find all sibling messages (sharing the same parent, same role). */
export function findSiblings(rows: MessageRow[], messageId: string): { siblings: MessageRow[]; index: number } {
  const msg = rows.find(r => r.id === messageId);
  if (!msg || !msg.parent_message_id) return { siblings: msg ? [msg] : [], index: 0 };

  const siblings = rows
    .filter(r => r.parent_message_id === msg.parent_message_id && r.role === msg.role)
    .sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());

  return { siblings, index: siblings.findIndex(s => s.id === messageId) };
}

// ── History conversion (MessageRow[] → ChatMessage[]) ───────────────────────

/**
 * Converts flat database rows into structured ChatMessage objects.
 * Implements "Virtual Merging" (Stage 2): consecutive assistant/tool blocks
 * from the same agent are merged into a single visual message to ensure
 * stable tool grouping and consistent identity.
 */
export function convertHistory(
  rows: MessageRow[],
  isAgentStreaming?: boolean,
  selectedBranches?: Record<string, string>,
): ChatMessage[] {
  // When branching data exists and selectedBranches provided, resolve active path first
  const resolved = selectedBranches && rows.some(r => r.parent_message_id !== null)
    ? resolveActivePath(rows, selectedBranches)
    : rows;

  // Filter out streaming placeholder messages ONLY if we have an active live stream
  // that will provide the same content. If not, show them as fallback (history).
  const filtered = resolved.filter(m => {
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
        parentMessageId: m.parent_message_id ?? undefined,
        branchFromMessageId: m.branch_from_message_id ?? undefined,
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
        parentMessageId: m.parent_message_id ?? undefined,
        branchFromMessageId: m.branch_from_message_id ?? undefined,
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
function getCachedHistoryMessages(sessionId: string | null, selectedBranches?: Record<string, string>): ChatMessage[] {
  if (!sessionId) return [];
  const cached = queryClient.getQueryData<{ messages: MessageRow[] }>(qk.sessionMessages(sessionId));
  return cached ? convertHistory(cached.messages, false, selectedBranches) : [];
}

/** Get all raw MessageRow[] from React Query cache for a session (for sibling discovery). */
export function getCachedRawMessages(sessionId: string | null): MessageRow[] {
  if (!sessionId) return [];
  const cached = queryClient.getQueryData<{ messages: MessageRow[] }>(qk.sessionMessages(sessionId));
  return cached?.messages ?? [];
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
      : getCachedHistoryMessages(sessionId, existingSt?.selectedBranches);

    update(agent, {
      streamError: null,
      connectionPhase: "streaming",
      connectionError: null,
      messageSource: { mode: "live", messages: seedMessages },
    });

    const token = getToken();

    const resumeHeaders: Record<string, string> = { Authorization: `Bearer ${token}` };
    const lastEid = getLastEventId(agent);
    if (lastEid) {
      resumeHeaders["Last-Event-ID"] = lastEid;
    }

    fetch(`/api/chat/${sessionId}/stream`, {
      method: "GET",
      headers: resumeHeaders,
      signal: controller.signal,
    })
      .then((resp) => {
        if (resp.status === 204) {
          // No active stream — engine already finished.
          // Transition to history mode so useSessionMessages fetches fresh data (Fix B).
          clearLastEventId(agent);
          update(agent, { connectionPhase: "idle", messageSource: { mode: "history", sessionId }, reconnectAttempt: 0 });
          return;
        }
        if (resp.status === 410) {
          // Stream expired — fall back to history mode without error
          clearLastEventId(agent);
          update(agent, {
            connectionPhase: "idle",
            messageSource: { mode: "history", sessionId },
            reconnectAttempt: 0,
          });
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
      update(agent, { connectionPhase: "idle", reconnectAttempt: 0 });
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
    update(agent, { connectionPhase: "reconnecting", connectionError: null, reconnectAttempt: attempt + 1 });
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
    clearLastEventId(agent);
    update(agent, { streamGeneration: (get().agents[agent]?.streamGeneration ?? 0) + 1, reconnectAttempt: 0 });
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
    let currentStepId: string | null = null;
    let currentStepGroup: StepGroupPart | null = null;
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

      // Get parser's current text/reasoning parts (emitted so far + buffered accum).
      // Use flush() to get complete snapshot including buffered chars.
      // We DON'T reset the parser here — flush() returns normalized copy.
      const textParts = incrementalParser.snapshot();
      // Non-text parts (tools, files, rich-cards) are in `parts` local array.
      const nonTextParts = parts.filter(p => p.type !== "text" && p.type !== "reasoning");

      set((draft) => {
        const st = draft.agents[agent];
        if (!st) return;
        if (st.messageSource.mode !== "live") {
          st.messageSource = { mode: "live", messages: [] };
        }
        const liveMessages = st.messageSource.messages;
        const existing = liveMessages.findIndex((m: ChatMessage) => m.id === assistantId);

        // Merge: text/reasoning from parser + tools/files from parts array.
        // Text comes first, then tools — this matches SSE event order for simple responses.
        // For interleaved text-tool-text, flushText() snapshots text into `parts` before
        // tool insertion, so nonTextParts includes tools at correct positions relative to
        // the flushed text parts that are also in `parts`.
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

        let currentEventId: string | null = null;
        let skipNextData = false;
        for (const line of lines) {
          // NET-02: Extract SSE event IDs for dedup and Last-Event-ID header
          const eid = extractSseEventId(line);
          if (eid !== null) {
            currentEventId = eid;
            const lastId = getLastEventId(agent);
            if (lastId !== null && parseInt(eid, 10) <= parseInt(lastId, 10)) {
              skipNextData = true;
            } else {
              skipNextData = false;
              setLastEventId(agent, eid);
            }
            continue;
          }
          if (!line.startsWith("data:")) continue;
          if (skipNextData) { skipNextData = false; continue; } // dedup
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

            case "step-start": {
              currentStepId = event.stepId;
              currentStepGroup = {
                type: "step-group",
                stepId: event.stepId,
                toolParts: [],
                isStreaming: true,
              };
              break;
            }

            case "step-finish": {
              if (currentStepGroup) {
                currentStepGroup.finishReason = event.finishReason;
                currentStepGroup.isStreaming = false;
                // Only push step-group part if it has tool calls.
                // Text-only steps (no tools) pass through as normal text.
                if (currentStepGroup.toolParts.length > 0) {
                  flushText();
                  parts.push(currentStepGroup);
                  scheduleUpdate();
                }
              }
              currentStepId = null;
              currentStepGroup = null;
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
              if (currentStepGroup) {
                currentStepGroup.toolParts.push(parts[parts.length - 1] as ToolPart);
              }
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

            case "tool-approval-needed": {
              flushText();
              parts.push({
                type: "approval",
                approvalId: event.approvalId,
                toolName: event.toolName,
                toolInput: event.toolInput,
                timeoutMs: event.timeoutMs,
                receivedAt: Date.now(),
                status: "pending",
              });
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
                  ...(event.modifiedInput != null ? { modifiedInput: event.modifiedInput } : {}),
                };
              }
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

              if (event.continuation) {
                // Continuation: do NOT reset state — keep accumulating into same message.
                // Push a separator part so UI renders the visual break.
                parts.push({ type: "continuation-separator" });
                incrementalParser.reset();
                // Do NOT reset assistantId, parts, or createdAt — message continues.
              } else {
                // Normal finish: reset for next agent turn (existing behavior).
                // FSM-04: Reset incremental parser state so next agent turn starts clean.
                incrementalParser.reset();
                // CRITICAL for multi-agent turn loop: reset state for next agent turn.
                assistantId = uuid();
                assistantCreatedAt = new Date().toISOString();
                parts = [];
              }
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
          update(agent, {
            connectionPhase: "idle",
            connectionError: null,
            pendingTargetAgent: null,
            turnCount: 0,
            reconnectAttempt: 0,
            // Keep messageSource as "live" with final messages — avoids flash when
            // React Query hasn't yet fetched fresh history.
          });

          // OPTI-03: Delayed transition to history mode.
          // Live messages remain visible while React Query fetches fresh history.
          // After 600ms (enough for cache invalidation + fetch), switch to history.
          const sid = receivedSessionId ?? get().agents[agent]?.activeSessionId;
          if (sid) {
            setTimeout(() => {
              const st = get().agents[agent];
              // Only transition if still in live mode for this session (user hasn't navigated away)
              if (st && st.messageSource.mode === "live" && st.activeSessionId === sid) {
                const liveMessages = st.messageSource.messages;
                const cachedData = queryClient.getQueryData<{ messages: MessageRow[] }>(
                  qk.sessionMessages(sid),
                );
                if (cachedData?.messages) {
                  const historyMessages = convertHistory(cachedData.messages);
                  const result = reconcileLiveWithHistory(liveMessages, historyMessages);
                  if (result === null) {
                    // OPTI-03: Content identical — just flip mode without changing rendered messages.
                    // This prevents any DOM mutation since message IDs match.
                  }
                  // History cache is populated — safe to transition
                  update(agent, { messageSource: { mode: "history", sessionId: sid } });
                }
                // If cachedData is not yet available, do NOT transition — stay in live mode.
                // React Query invalidation will eventually populate the cache, and
                // ChatThread's sourceMessages fallback handles this gracefully.
              }
            }, 600);
          }
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
        seedMessages = getCachedHistoryMessages(sessionId, st.selectedBranches);
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
        messages = getCachedHistoryMessages(sessionId, st.selectedBranches);
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
        messages = getCachedHistoryMessages(sessionId, st.selectedBranches);
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

    switchBranch: (parentMessageId: string, selectedChildId: string) => {
      const agent = get().currentAgent;
      const st = get().agents[agent];
      if (!st) return;

      set((draft) => {
        const s = draft.agents[agent];
        if (s) s.selectedBranches[parentMessageId] = selectedChildId;
      });

      // Re-resolve display messages from cached history rows
      if (st.messageSource.mode === "history" && st.activeSessionId) {
        // Invalidate React Query cache to trigger re-render with new branch selection
        // The component useMemo will re-run convertHistory with updated selectedBranches
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(st.activeSessionId) });
      }
    },

    forkAndRegenerate: async (messageId: string, newContent: string) => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();
      const sessionId = st.activeSessionId;
      if (!sessionId) return;

      try {
        const resp = await apiPost<{
          message_id: string;
          parent_message_id: string;
          branch_from_message_id: string;
        }>(`/api/sessions/${sessionId}/fork`, {
          branch_from_message_id: messageId,
          content: newContent,
        });

        // Build seed: active path up to (but not including) the forked message, then new user msg
        const currentSt = get().agents[agent] ?? emptyAgentState();
        let messages: ChatMessage[];
        if (currentSt.messageSource.mode === "history") {
          messages = getCachedHistoryMessages(sessionId, currentSt.selectedBranches);
        } else {
          messages = getLiveMessages(currentSt.messageSource);
        }

        const forkIdx = messages.findIndex((m) => m.id === messageId);
        const seedMessages = forkIdx >= 0 ? messages.slice(0, forkIdx) : messages;

        // Update selectedBranches to select the new branch
        set((draft) => {
          const s = draft.agents[agent];
          if (s && resp.parent_message_id) {
            s.selectedBranches[resp.parent_message_id] = resp.message_id;
          }
        });

        startStream(agent, sessionId, seedMessages, newContent);
      } catch (e) {
        console.error("[fork] failed:", e);
      }
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
        : getCachedHistoryMessages(st.activeSessionId, st.selectedBranches);
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

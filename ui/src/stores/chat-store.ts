import { create } from "zustand";
import { devtools } from "zustand/middleware";
import { immer } from "zustand/middleware/immer";

import { apiGet, apiDelete, apiPatch, getToken } from "@/lib/api";
import { parseSSELines, parseSseEvent } from "@/stores/sse-events";
import type { SessionRow, MessageRow } from "@/types/api";
import { queryClient } from "@/lib/query-client";
import { qk } from "@/lib/queries";

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Generate UUID — fallback for non-secure contexts (HTTP) where crypto.randomUUID is unavailable */
function uuid(): string {
  if (typeof crypto !== "undefined" && crypto.randomUUID) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
  });
}

// ── Constants ────────────────────────────────────────────────────────────────

const SESSIONS_PAGE_SIZE = 40;
const MESSAGES_HISTORY_LIMIT = 100;
export const MAX_INPUT_LENGTH = 32_000;
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
  input: unknown;
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
}

// ── Stream status ───────────────────────────────────────────────────────────

export type StreamStatus = "idle" | "submitted" | "streaming" | "error";

export function isActiveStream(status: StreamStatus | undefined): boolean {
  return status === "submitted" || status === "streaming";
}

// ── Per-agent state ─────────────────────────────────────────────────────────

interface AgentState {
  activeSessionId: string | null;
  /** Messages from the current live stream (including seeded history). */
  liveMessages: ChatMessage[];
  /** Whether we're viewing a DB snapshot (true) or live stream (false). */
  viewMode: "history" | "live";
  streamStatus: StreamStatus;
  streamError: string | null;
  /** When true, next sendMessage will force backend to create a new session. */
  forceNewSession: boolean;
  /** Session ID when agent is processing from another channel (Telegram, cron). */
  thinkingSessionId: string | null;
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
}

function emptyAgentState(): AgentState {
  return {
    activeSessionId: null,
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
}

// ── Store interface ─────────────────────────────────────────────────────────

const LAST_SESSION_KEY = "hydeclaw.chat.lastSession";

// Per-agent abort controllers (keyed by agent name, not module-scoped)
const agentAbortControllers: Record<string, AbortController | null> = {};

// Stream generation counter — prevents stale SSE deltas from writing to wrong session
// after session switch. Incremented on each startStream(), checked in pushUpdate().
let streamGeneration = 0;

// Stale stream detection — abort if no SSE data received within this window.
// Prevents streamStatus from getting stuck in "streaming" when the connection hangs
// (e.g. LLM stall, network break) and reader.read() blocks indefinitely.
const STREAM_STALE_TIMEOUT_MS = 60_000;
const staleStreamTimers: Record<string, ReturnType<typeof setInterval> | null> = {};
const staleAbortedFlags: Record<string, boolean> = {};

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

/** Extract <think> blocks into reasoning parts and clean text parts from raw content */
function parseContentParts(raw: string): MessagePart[] {
  if (!raw) return [];
  const parts: MessagePart[] = [];
  const thinkRegex = /<think>([\s\S]*?)<\/think>/g;
  let lastIndex = 0;
  let match;

  while ((match = thinkRegex.exec(raw)) !== null) {
    const before = raw.slice(lastIndex, match.index).trim();
    if (before) parts.push({ type: "text", text: before });
    const reasoning = match[1].trim();
    if (reasoning) parts.push({ type: "reasoning", text: reasoning });
    lastIndex = match.index + match[0].length;
  }

  // Handle remaining text after last closed </think> (or all text if no <think> blocks)
  let after = raw.slice(lastIndex).trim();
  // Handle unclosed <think> at end of remaining text
  const unclosedIdx = after.indexOf("<think>");
  if (unclosedIdx >= 0) {
    const beforeUnclosed = after.slice(0, unclosedIdx).trim();
    if (beforeUnclosed) {
      const cleanedBefore = beforeUnclosed
        .replace(/<minimax:tool_call>[\s\S]*?(<\/minimax:tool_call>|$)\s*/g, "")
        .replace(/\[TOOL_CALL\][\s\S]*?(\[\/TOOL_CALL\]|$)\s*/g, "")
        .trim();
      if (cleanedBefore) parts.push({ type: "text", text: cleanedBefore });
    }
    const unclosedReasoning = after.slice(unclosedIdx + 7).trim();
    if (unclosedReasoning) parts.push({ type: "reasoning", text: unclosedReasoning });
  } else {
    const cleaned = after
      .replace(/<minimax:tool_call>[\s\S]*?(<\/minimax:tool_call>|$)\s*/g, "")
      .replace(/\[TOOL_CALL\][\s\S]*?(\[\/TOOL_CALL\]|$)\s*/g, "")
      .trim();
    if (cleaned) parts.push({ type: "text", text: cleaned });
  }

  return parts.length > 0 ? parts : [{ type: "text", text: raw.trim() }];
}

// ── History conversion (MessageRow[] → ChatMessage[]) ───────────────────────

export function convertHistory(rows: MessageRow[]): ChatMessage[] {
  // Filter out streaming placeholder messages — they duplicate content
  // and show a separate loading indicator that conflicts with ThinkingMessage.
  const filtered = rows.filter(m => m.status !== "streaming");

  const toolCallMap = new Map<string, { name: string; arguments: unknown }>();
  for (const m of filtered) {
    if (m.role === "assistant" && m.tool_calls) {
      const calls = m.tool_calls as Array<{
        id: string;
        name: string;
        arguments?: unknown;
      }>;
      if (Array.isArray(calls)) {
        for (const tc of calls) {
          if (tc.id && tc.name) {
            toolCallMap.set(tc.id, {
              name: tc.name,
              arguments: tc.arguments ?? {},
            });
          }
        }
      }
    }
  }

  const messages: ChatMessage[] = [];
  let currentAssistant: ChatMessage | null = null;

  for (const m of filtered) {
    if (m.role === "user") {
      if (currentAssistant) {
        messages.push(currentAssistant);
        currentAssistant = null;
      }
      messages.push({
        id: m.id,
        role: "user",
        parts: [{ type: "text", text: m.content }],
        createdAt: m.created_at,
        agentId: m.agent_id ?? undefined,
      });
    } else if (m.role === "assistant" && !m.tool_call_id) {
      // D-01: No merging. Each assistant DB row becomes its own ChatMessage.
      const newParts = parseContentParts(m.content);
      if (currentAssistant) {
        messages.push(currentAssistant);
      }
      currentAssistant = {
        id: m.id,
        role: "assistant",
        parts: newParts,
        createdAt: m.created_at,
        agentId: m.agent_id ?? undefined,
      };
    } else if (m.role === "tool" && m.tool_call_id) {
      if (currentAssistant) {
        const tc = toolCallMap.get(m.tool_call_id);

        // Extract __file__: markers from tool content for inline image display
        const lines = (m.content || "").split("\n");
        const cleanLines: string[] = [];
        for (const line of lines) {
          if (line.startsWith("__file__:")) {
            try {
              const meta = JSON.parse(line.slice("__file__:".length));
              if (meta.url && meta.mediaType?.startsWith("image/")) {
                currentAssistant.parts.push({
                  type: "file",
                  url: meta.url,
                  mediaType: meta.mediaType,
                });
              }
            } catch { /* ignore malformed markers */ }
          } else {
            cleanLines.push(line);
          }
        }

        currentAssistant.parts.push({
          type: "tool",
          toolCallId: m.tool_call_id,
          toolName: tc?.name || "tool",
          state: "output-available",
          input: tc?.arguments ?? {},
          output: cleanLines.join("\n"),
        });
      }
    }
  }
  if (currentAssistant) messages.push(currentAssistant);

  return messages
    .filter((m) => m.parts.length > 0)
    .filter((m) =>
      m.parts.some((p) => p.type !== "text" || (p.type === "text" && p.text.trim() !== ""))
    );
}

/** Get history messages from React Query cache, or empty array if not cached. */
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
      const streamStatus = st.streamStatus === "submitted" ? "streaming" : st.streamStatus;
      apiPatch(`/api/sessions/${st.activeSessionId}`, {
        ui_state: { viewMode: st.viewMode, streamStatus },
      }).catch((e) => { console.warn("[chat] save failed:", e); });
    }, 500);
  }

  /** Abort active stream for an agent and reset status. */
  function abortActiveStream(agent: string) {
    if (staleStreamTimers[agent]) {
      clearInterval(staleStreamTimers[agent]!);
      staleStreamTimers[agent] = null;
    }
    staleAbortedFlags[agent] = false;
    if (agentAbortControllers[agent]) {
      agentAbortControllers[agent].abort();
      agentAbortControllers[agent] = null;
      update(agent, { streamStatus: "idle" });
    }
  }

  // ── SSE stream handler ──
  function startStream(agent: string, sessionId: string | null, messages: ChatMessage[], userText: string) {
    abortActiveStream(agent);
    streamGeneration++;
    const myGeneration = streamGeneration;
    const controller = new AbortController();
    agentAbortControllers[agent] = controller;

    // Build user message
    const userMsg: ChatMessage = {
      id: uuid(),
      role: "user",
      parts: [{ type: "text", text: userText }],
      createdAt: new Date().toISOString(),
    };
    const allMessages = [...messages, userMsg];
    update(agent, {
      liveMessages: allMessages,
      viewMode: "live",
      streamStatus: "submitted",
      streamError: null,
      agentTurns: [],  // Reset for new stream
      turnCount: 0,
      turnLimitMessage: null,
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
        if (err.name === "AbortError") {
          // Stale stream abort — show error so user knows what happened
          if (staleAbortedFlags[agent]) {
            update(agent, {
              streamStatus: "error",
              streamError: "Stream timed out — no response for 60s",
            });
            saveUiState(agent);
          }
          return;
        }
        update(agent, {
          streamStatus: "error",
          streamError: err.message || "Stream failed",
        });
        saveUiState(agent);
      });
  }

  async function processSSEStream(
    agent: string,
    body: ReadableStream<Uint8Array>,
    signal: AbortSignal,
    generation: number,
  ) {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    const buffer = { current: "" };

    // Stale stream detection: track last received data timestamp
    let lastDataAt = Date.now();
    staleAbortedFlags[agent] = false;
    staleStreamTimers[agent] = setInterval(() => {
      if (Date.now() - lastDataAt > STREAM_STALE_TIMEOUT_MS) {
        console.warn(`[chat] SSE stream stale for ${STREAM_STALE_TIMEOUT_MS / 1000}s, aborting`);
        staleAbortedFlags[agent] = true;
        agentAbortControllers[agent]?.abort();
      }
    }, 5_000);

    // Mutable assistant message being built
    let assistantId = uuid();
    let assistantCreatedAt = new Date().toISOString();
    let parts: MessagePart[] = [];
    let currentTextId: string | null = null;
    let textAccum = "";
    let insideThink = false; // tracking <think> state across deltas
    let reasoningAccum = ""; // reasoning text accumulator
    const toolInputChunks = new Map<string, string[]>();
    let receivedSessionId: string | null = null;
    // Initialize from pendingTargetAgent so first render shows correct avatar
    let currentRespondingAgent: string | null = get().agents[agent]?.pendingTargetAgent ?? null;

    function flushText() {
      if (!currentTextId) return;
      // Parse accumulated text for <think> blocks
      let remaining = textAccum;
      while (remaining) {
        if (insideThink) {
          const endIdx = remaining.indexOf("</think>");
          if (endIdx >= 0) {
            reasoningAccum += remaining.slice(0, endIdx);
            if (reasoningAccum.trim()) {
              // Merge with previous reasoning part if exists
              const lastPart = parts[parts.length - 1];
              if (lastPart && lastPart.type === "reasoning") {
                parts[parts.length - 1] = { type: "reasoning", text: lastPart.text + reasoningAccum.trim() };
              } else {
                parts.push({ type: "reasoning", text: reasoningAccum.trim() });
              }
            }
            reasoningAccum = "";
            insideThink = false;
            remaining = remaining.slice(endIdx + 8); // skip "</think>"
          } else {
            // Still inside think, accumulate
            reasoningAccum += remaining;
            // Update reasoning part in-place for streaming display (replace, don't concatenate)
            const lastPart = parts[parts.length - 1];
            if (lastPart && lastPart.type === "reasoning") {
              parts[parts.length - 1] = { type: "reasoning", text: reasoningAccum.trim() };
            } else if (reasoningAccum.trim()) {
              parts.push({ type: "reasoning", text: reasoningAccum.trim() });
            }
            remaining = "";
          }
        } else {
          const startIdx = remaining.indexOf("<think>");
          if (startIdx >= 0) {
            const before = remaining.slice(0, startIdx);
            if (before.trim()) {
              const lastPart = parts[parts.length - 1];
              if (lastPart && lastPart.type === "text") {
                parts[parts.length - 1] = { type: "text", text: lastPart.text + before };
              } else {
                parts.push({ type: "text", text: before });
              }
            }
            insideThink = true;
            reasoningAccum = "";
            remaining = remaining.slice(startIdx + 7); // skip "<think>"
          } else {
            // No think tags, plain text
            if (remaining.trim()) {
              const lastPart = parts[parts.length - 1];
              if (lastPart && lastPart.type === "text") {
                parts[parts.length - 1] = { type: "text", text: lastPart.text + remaining };
              } else {
                parts.push({ type: "text", text: remaining });
              }
            }
            remaining = "";
          }
        }
      }
      currentTextId = null;
      textAccum = "";
    }

    function pushUpdate() {
      // Guard: stale stream — a newer stream has started, discard updates
      if (generation !== streamGeneration) return;
      // Guard: don't update store after abort (prevents race with stopStream)
      if (signal.aborted) return;
      const assistantMsg: ChatMessage = {
        id: assistantId,
        role: "assistant",
        parts: [...parts],
        createdAt: assistantCreatedAt,
        agentId: currentRespondingAgent ?? undefined,
      };
      // Immer in-place mutation: only assistantMsg gets a new object reference.
      // All other liveMessages keep their refs → WeakMap cache hits in convertMessage.
      set((draft) => {
        const st = draft.agents[agent];
        if (!st) return;
        const existing = st.liveMessages.findIndex((m: ChatMessage) => m.id === assistantId);
        if (existing >= 0) {
          st.liveMessages[existing] = assistantMsg;
        } else {
          st.liveMessages.push(assistantMsg);
        }
        if (st.streamStatus !== "error") st.streamStatus = "streaming";
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
      }, 50);
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
        lastDataAt = Date.now();

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
              if (sid && generation === streamGeneration) {
                receivedSessionId = sid;
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
              if (stNow) {
                const deduped = stNow.liveMessages.filter(
                  (m) => m.id !== newId && !m.id.startsWith("resume-")
                );
                if (deduped.length !== stNow.liveMessages.length) {
                  update(agent, { liveMessages: deduped });
                }
              }
              break;
            }

            case "text-start": {
              flushText();
              currentTextId = event.id ?? "";
              textAccum = "";
              if (event.agentName) currentRespondingAgent = event.agentName;
              break;
            }

            case "text-delta": {
              textAccum += event.delta;
              scheduleUpdate();
              break;
            }

            case "text-end": {
              flushText();
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
                parts[idx] = { ...(parts[idx] as ToolPart), state: "input-available", input };
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
              // Full content sync from DB (resume after disconnect/restart)
              const { content: syncContent, toolCalls: syncToolCalls, status: syncStatus } = event;

              const syncParts: MessagePart[] = [];
              if (syncContent) {
                syncParts.push({ type: "text", text: syncContent });
              }
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

              const syncMsg: ChatMessage = {
                id: assistantId,
                role: "assistant",
                parts: syncParts.length > 0 ? syncParts : [{ type: "text", text: "" }],
                createdAt: assistantCreatedAt,
              };

              // Replace live messages: keep user messages + add sync assistant message
              const stSync = get().agents[agent];
              if (stSync) {
                const userMsgs = stSync.liveMessages.filter(
                  (m) => m.role === "user"
                );
                update(agent, { liveMessages: [...userMsgs, syncMsg] });
              }

              if (syncStatus === "finished" || syncStatus === "error" || syncStatus === "interrupted") {
                const errorText = syncStatus === "error" ? (event.error ?? null) : null;
                // Don't reset to idle during turn loop — pendingTargetAgent means more agents coming
                const inTurnLoop = !!get().agents[agent]?.pendingTargetAgent;
                if (syncStatus === "error" || !inTurnLoop) {
                  update(agent, {
                    streamStatus: syncStatus === "error" ? "error" : "idle",
                    streamError: errorText,
                  });
                }
              }
              break;
            }

            case "finish": {
              // Cancel any pending update and do synchronous update
              cancelScheduledUpdate();
              flushText();
              pushUpdate();
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
                update(agent, { streamStatus: "error", streamError: errText });
              }
              break;
            }
          }
        }
      }
    } finally {
      if (staleStreamTimers[agent]) {
        clearInterval(staleStreamTimers[agent]!);
        staleStreamTimers[agent] = null;
      }
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
        // Preserve error status if error event was already received
        const isError = get().agents[agent]?.streamStatus === "error";
        if (!isError) {
          update(agent, { streamStatus: "idle", pendingTargetAgent: null, turnCount: 0 });
        }
        saveUiState(agent);
        // SSE stream finished — clear activeSessionIds for this session so the
        // thinking indicator stops even if WS agent_processing "end" is delayed or lost.
        const finishedSessionId = get().agents[agent]?.activeSessionId;
        if (finishedSessionId) {
          get().markSessionInactive(agent, finishedSessionId);
        }
        // Session status is primarily server-driven via WS agent_processing events;
        // the markSessionInactive call above is a fallback for delayed/lost WS events.
      } else if (parts.length > 0) {
        // On abort: save partial response to liveMessages.
        // Status set by caller: stopStream → "idle", stale abort → "error" (via .catch)
        const st = get().agents[agent];
        if (st) {
          const assistantMsg: ChatMessage = {
            id: assistantId,
            role: "assistant",
            parts: [...parts],
            createdAt: assistantCreatedAt,
            agentId: currentRespondingAgent ?? undefined,
          };
          const existing = st.liveMessages.findIndex((m) => m.id === assistantId);
          const updated =
            existing >= 0
              ? st.liveMessages.map((m, i) => (i === existing ? assistantMsg : m))
              : [...st.liveMessages, assistantMsg];
          update(agent, { liveMessages: updated });
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

      // Check if current session is multi-agent and includes the new agent
      const prevState = get().agents[prev];
      const activeSessionId = prevState?.activeSessionId;

      if (activeSessionId) {
        const participants = get().sessionParticipants[activeSessionId];
        if (participants && participants.includes(name)) {
          // Carry over the session — the new agent is already a participant
          ensure(name);
          update(name, {
            activeSessionId,
            liveMessages: prevState?.liveMessages ?? [],
            viewMode: prevState?.viewMode ?? "live",
            streamStatus: prevState?.streamStatus ?? "idle",
          });
          set({ currentAgent: name });
          saveLastSession(name, activeSessionId);
          return;
        }
      }

      // Abort stream for the agent being left
      if (agentAbortControllers[prev]) {
        agentAbortControllers[prev].abort();
        agentAbortControllers[prev] = null;
        update(prev, { streamStatus: "idle" });
      }
      ensure(name);
      // Immediately reset to new-chat state so no stale session is shown during render.
      // The restore effect in page.tsx may later select a server-active session.
      update(name, {
        activeSessionId: null,
        viewMode: "live",
        liveMessages: [],
        streamStatus: "idle",
        streamError: null,
        forceNewSession: true,
      });
      set({ currentAgent: name });
      // Save agent to localStorage and clear any stale session ID for this agent
      // (prevents cross-agent contamination when switching)
      clearLastSessionId(name);
      saveLastSession(name);
      // Sessions list is managed by React Query (useSessions hook)
      queryClient.invalidateQueries({ queryKey: qk.sessions(name) });
    },

    selectSession: async (sessionId: string, forAgent?: string) => {
      const agent = forAgent ?? get().currentAgent;
      ensure(agent);

      // If re-selecting the same session that's currently streaming, just switch to live view
      const currentState = get().agents[agent];
      if (currentState?.activeSessionId === sessionId && isActiveStream(currentState.streamStatus)) {
        update(agent, { viewMode: "live" });
        return;
      }

      abortActiveStream(agent);

      update(agent, {
        activeSessionId: sessionId,
        viewMode: "history",
        forceNewSession: false,
        liveMessages: [],
        streamStatus: "idle",
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
      if (agentAbortControllers[agent]) {
        agentAbortControllers[agent].abort();
        agentAbortControllers[agent] = null;
      }
      update(agent, {
        activeSessionId: sessionId,
        viewMode: "history",
        forceNewSession: false,
        liveMessages: [],
        streamStatus: "idle",
      });
      saveLastSession(agent, sessionId);
      // Data fetching handled by useSessionMessages() React Query hook
    },

    newChat: () => {
      const agent = get().currentAgent;
      agentAbortControllers[agent]?.abort();
      agentAbortControllers[agent] = null;
      update(agent, {
        activeSessionId: null,
        viewMode: "live",
        liveMessages: [],
        streamStatus: "idle",
        streamError: null,
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
      const updates: Partial<AgentState> = { thinkingSessionId: sessionId };

      // On reload (before restore): Zustand activeSessionId is null — set it so
      // useSessionMessages can fetch and the DB streaming record is visible.
      // Guard: only when null AND not in "new chat" mode — don't override newChat().
      if (sessionId !== null && st?.activeSessionId == null && !st?.forceNewSession) {
        updates.activeSessionId = sessionId;
      }

      update(agent, updates);
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

    sendMessage: (text: string) => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();

      if (isActiveStream(st.streamStatus)) return;

      // Parse @-mention to set pendingTargetAgent for thinking indicator
      const mentionMatch = text.match(/^@(\S+)/);
      const targetAgent = mentionMatch ? mentionMatch[1] : null;
      update(agent, { pendingTargetAgent: targetAgent });

      let sessionId = st.activeSessionId;
      let seedMessages: ChatMessage[] = [];

      if (st.viewMode === "history") {
        // Continue from history — get messages from React Query cache
        seedMessages = getCachedHistoryMessages(sessionId);
        update(agent, { viewMode: "live" });
      } else if (st.liveMessages.length > 0) {
        seedMessages = st.liveMessages;
      }

      startStream(agent, sessionId, seedMessages, text);
    },

    stopStream: () => {
      const agent = get().currentAgent;
      agentAbortControllers[agent]?.abort();
      agentAbortControllers[agent] = null;
      update(agent, { streamStatus: "idle" });
    },

    regenerate: () => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();

      // Abort any active stream first
      if (isActiveStream(st.streamStatus)) {
        agentAbortControllers[agent]?.abort();
        agentAbortControllers[agent] = null;
        update(agent, { streamStatus: "idle" });
      }

      let sessionId = st.activeSessionId;
      let messages: ChatMessage[];

      if (st.viewMode === "history") {
        messages = getCachedHistoryMessages(sessionId);
        update(agent, { viewMode: "live" });
      } else {
        messages = st.liveMessages;
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

      if (isActiveStream(st.streamStatus)) {
        agentAbortControllers[agent]?.abort();
        agentAbortControllers[agent] = null;
        update(agent, { streamStatus: "idle" });
      }

      let sessionId = st.activeSessionId;
      let messages: ChatMessage[];

      if (st.viewMode === "history") {
        messages = getCachedHistoryMessages(sessionId);
        update(agent, { viewMode: "live" });
      } else {
        messages = st.liveMessages;
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
        agentAbortControllers[agent]?.abort();
        agentAbortControllers[agent] = null;
        update(agent, {
          activeSessionId: null, viewMode: "live", liveMessages: [],
          streamStatus: "idle", streamError: null, forceNewSession: true,
        });
        saveLastSession(agent);
      }
      queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
    },

    deleteAllSessions: async () => {
      const agent = get().currentAgent;
      await apiDelete(`/api/sessions?agent=${encodeURIComponent(agent)}`);
      // Use captured `agent` — currentAgent may have changed during await
      agentAbortControllers[agent]?.abort();
      agentAbortControllers[agent] = null;
      update(agent, {
        activeSessionId: null, viewMode: "live", liveMessages: [],
        streamStatus: "idle", streamError: null, forceNewSession: true,
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
      if (st.viewMode === "history" && st.activeSessionId) {
        // Invalidate React Query cache to reload history
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(st.activeSessionId) });
      } else {
        update(agent, {
          liveMessages: st.liveMessages.filter((m) => m.id !== messageId),
        });
      }
    },

    exportSession: async () => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent];
      if (!st) return;

      const messages = st.viewMode === "live"
        ? st.liveMessages
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

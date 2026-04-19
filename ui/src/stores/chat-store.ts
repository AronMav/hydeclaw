import { create } from "zustand";
import { devtools } from "zustand/middleware";
import { immer } from "zustand/middleware/immer";

import { apiDelete, apiPatch, apiPost } from "@/lib/api";
import { queryClient } from "@/lib/query-client";
import { qk } from "@/lib/queries";

import { isActivePhase, emptyAgentState, getLiveMessages } from "./chat-types";
import type { ChatMessage, TextPart, AgentState, ChatStore } from "./chat-types";
import { getCachedHistoryMessages } from "./chat-history";
import { createStreamingRenderer } from "./streaming-renderer";
import { saveLastSession, clearLastSessionId } from "./chat-persistence";

// ── Re-exports for backward compatibility ───────────────────────────────────
export type { ChatMessage, MessagePart, TextPart, ToolPart, ToolPartState, RichCardPart, FilePart, SourceUrlPart, ReasoningPart, ConnectionPhase, MessageSource, ChatStore, ApprovalPart, StepGroupPart, ContinuationSeparatorPart } from "./chat-types";
export { isActivePhase, MAX_INPUT_LENGTH, STREAM_THROTTLE_MS } from "./chat-types";
export { convertHistory, getCachedHistoryMessages, getCachedRawMessages, findSiblings } from "./chat-history";
export { contentHash, reconcileLiveWithHistory } from "./chat-reconciliation";
export { saveLastSession, getInitialAgent, getLastSessionId } from "./chat-persistence";

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

  // ── Streaming renderer (SSE processing, rAF throttling, reconnection) ──
  const renderer = createStreamingRenderer({ get, set });
  // Wire saveLastSession callback (avoids circular dependency)
  renderer.onSessionId((agent: string, sessionId: string) => {
    saveLastSession(agent, sessionId);
  });

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
          // Multi-agent session reuse: the new agent inherits the same
          // sessionId. Invalidate so React Query refetches the fresh DB
          // state under the new agent's query context.
          queryClient.invalidateQueries({ queryKey: qk.sessionMessages(activeSessionId) });
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

      // User-initiated agent switch to a DIFFERENT session (or no shared
      // session). Invalidate the previous agent's session so returning to
      // it later shows fresh data.
      if (activeSessionId) {
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(activeSessionId) });
      }
      // MEM-01: clean up all Maps for previous agent
      renderer.cleanupAgent(prev);
      update(prev, { connectionPhase: "idle" });
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

      // Invalidate React Query cache for BOTH the previous active session
      // (its DB state may have changed after the aborted stream wrote partial
      // assistant text) AND the incoming session. Without this, returning to
      // a previously-streaming session showed stale cached data — the user's
      // initial message could be missing if the cache was populated before
      // the backend saved it. Regression 2026-04-18.
      const previousSessionId = currentState?.activeSessionId;
      if (previousSessionId && previousSessionId !== sessionId) {
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(previousSessionId) });
      }
      queryClient.invalidateQueries({ queryKey: qk.sessionMessages(sessionId) });

      // Local-only abort: tear down the UI fetch so the new session can
      // render, but DO NOT POST /abort to the backend. A POST here would
      // cancel the departing session's engine task — if its provider is
      // slow to acknowledge the cancel, the cancel-grace window exceeds
      // 30 s and the session gets marked `'interrupted'` in DB. The user
      // only switched tabs; they did not explicitly Stop. The backend
      // stream finishes on its own (10-minute SSE safety net covers
      // worst-case abandonment) and the completed response is waiting
      // when the user returns.
      renderer.abortLocalOnly(agent);

      update(agent, {
        activeSessionId: sessionId,
        messageSource: { mode: "history", sessionId },
        forceNewSession: false,
        renderLimit: 100,
      });
      saveLastSession(agent, sessionId);
    },

    selectSessionById: (agent: string, sessionId: string) => {
      // Switch to the agent and select the session
      set({ currentAgent: agent });
      ensure(agent);
      // Abort any active stream for this agent
      const currentState = get().agents[agent];
      const previousSessionId = currentState?.activeSessionId;
      if (previousSessionId && previousSessionId !== sessionId) {
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(previousSessionId) });
      }
      queryClient.invalidateQueries({ queryKey: qk.sessionMessages(sessionId) });
      // See selectSession above — navigation must not cancel the backend.
      renderer.abortLocalOnly(agent);
      update(agent, {
        activeSessionId: sessionId,
        messageSource: { mode: "history", sessionId },
        forceNewSession: false,
        connectionPhase: "idle",
      });
      saveLastSession(agent, sessionId);
    },

    newChat: () => {
      const agent = get().currentAgent;
      // Invalidate the departing session's React Query cache — the stream
      // we are detaching from may still write partial assistant text to
      // DB. Without this, returning to that session via the sidebar shows
      // stale data.
      const previousSessionId = get().agents[agent]?.activeSessionId;
      if (previousSessionId) {
        queryClient.invalidateQueries({ queryKey: qk.sessionMessages(previousSessionId) });
      }
      // Local-only abort: starting a new chat does not imply the user
      // wants to cancel the previous response — they may want to see it
      // completed when they come back. See selectSession for the full
      // rationale.
      renderer.abortLocalOnly(agent);
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
      const { getToken } = await import("@/lib/api");
      const token = getToken();
      await fetch(`/api/agents/${encodeURIComponent(agent)}/model-override`, {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({ model }),
      }).catch((e) => { console.warn("[chat] save failed:", e); }); // fail silently — store already updated optimistically
    },

    resumeStream: (agent: string, sessionId: string) => renderer.resumeStream(agent, sessionId),

    sendMessage: (text: string, attachments?: Array<any>) => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();

      if (isActivePhase(st.connectionPhase)) return;

      let sessionId = st.activeSessionId;
      let seedMessages: ChatMessage[] = [];

      if (st.messageSource.mode === "history") {
        // Continue from history — get messages from React Query cache.
        // Do NOT flip messageSource here; startStream sets messageSource atomically.
        seedMessages = getCachedHistoryMessages(sessionId, st.selectedBranches);
      } else if (st.messageSource.mode === "live" && st.messageSource.messages.length > 0) {
        seedMessages = st.messageSource.messages;
      }

      renderer.startStream(agent, sessionId, seedMessages, text, attachments);
    },

    stopStream: () => {
      const agent = get().currentAgent;
      // Clear any pending reconnect timer — user abort must not trigger reconnect
      const stopTimer = renderer.getReconnectTimer(agent);
      if (stopTimer) {
        clearTimeout(stopTimer);
        renderer.setReconnectTimer(agent, null);
      }
      renderer.getAbortCtrl(agent)?.abort();
      update(agent, { connectionPhase: "idle" });
    },

    regenerate: () => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();

      // Abort any active stream first
      if (isActivePhase(st.connectionPhase)) {
        renderer.abortActiveStream(agent);
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

      renderer.startStream(agent, sessionId, messages, userText);
    },

    regenerateFrom: (messageId: string) => {
      const store = get();
      const agent = store.currentAgent;
      const st = store.agents[agent] ?? emptyAgentState();

      if (isActivePhase(st.connectionPhase)) {
        renderer.abortActiveStream(agent);
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

      renderer.startStream(agent, sessionId, seedMessages, userText);
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
        renderer.abortActiveStream(agent);
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
      renderer.abortActiveStream(agent);
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

        const currentSt = get().agents[agent] ?? emptyAgentState();
        let messages: ChatMessage[];
        if (currentSt.messageSource.mode === "history") {
          messages = getCachedHistoryMessages(sessionId, currentSt.selectedBranches);
        } else {
          messages = getLiveMessages(currentSt.messageSource);
        }

        const forkIdx = messages.findIndex((m) => m.id === messageId);
        const seedMessages = forkIdx >= 0 ? messages.slice(0, forkIdx) : messages;

        set((draft) => {
          const s = draft.agents[agent];
          if (s && resp.parent_message_id) {
            s.selectedBranches[resp.parent_message_id] = resp.message_id;
          }
        });

        renderer.startStream(agent, sessionId, seedMessages, newContent);
      } catch (e) {
        console.error("[fork] failed:", e);
      }
    },
  };
    }),
    { name: "ChatStore", enabled: process.env.NODE_ENV !== "production" },
  ),
);


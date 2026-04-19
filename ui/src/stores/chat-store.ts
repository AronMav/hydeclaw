import { create } from "zustand";
import { devtools } from "zustand/middleware";
import { immer } from "zustand/middleware/immer";

import { apiDelete, apiPatch } from "@/lib/api";
import { queryClient } from "@/lib/query-client";
import { qk } from "@/lib/queries";

import { emptyAgentState, getLiveMessages } from "./chat-types";
import type { AgentState, ChatStore } from "./chat-types";
import { getCachedHistoryMessages } from "./chat-history";
import { createStreamingRenderer } from "./streaming-renderer";
import type { StreamingRenderer } from "./streaming-renderer";
import { saveLastSession } from "./chat-persistence";
import { createNavigationActions } from "./chat/actions/navigation";
import { createStreamActions } from "./chat/actions/stream-control";

// ── ActionDeps ──────────────────────────────────────────────────────────────
// Shared dependency bag passed to every action factory.
// Uses the same get/set closures that the immer factory provides — matching
// the existing codebase convention (no StoreApi adapter needed).
import type { QueryClient } from "@tanstack/react-query";

export type ActionDeps = {
  get: () => ChatStore;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  set: (updater: ((draft: any) => void) | Partial<ChatStore>) => void;
  queryClient: QueryClient;
  renderer: StreamingRenderer;
};

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

  // ── Action factories ─────────────────────────────────────────────────────
  const navigationActions = createNavigationActions({ get, set, queryClient, renderer });
  const streamActions = createStreamActions({ get, set, queryClient, renderer });

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

    ...navigationActions,
    ...streamActions,

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

  };
    }),
    { name: "ChatStore", enabled: process.env.NODE_ENV !== "production" },
  ),
);


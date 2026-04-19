"use client";

import { useChatStore } from "@/stores/chat-store";
import { useSessions } from "@/lib/queries";
import { isActivePhase } from "@/stores/chat-types";

/**
 * Single source of truth for "is the engine processing for this agent?".
 *
 * Combines three signals:
 *  - UI-side connectionPhase (store)
 *  - activeSessionIds (store) — WS-delivered "agent_processing" events
 *  - DB-side run_status === "running" (React Query)
 *
 * Policy: only ChatThread should use this hook. Children receive the
 * boolean as a prop. If a future consumer needs it elsewhere, use a
 * React context rather than multiplying subscriptions.
 */
export function useEngineRunning(agent: string): boolean {
  const activeSessionId = useChatStore((s) => s.agents[agent]?.activeSessionId ?? null);
  const connectionPhase = useChatStore((s) => s.agents[agent]?.connectionPhase ?? "idle");
  const activeSessionIds = useChatStore((s) => s.agents[agent]?.activeSessionIds ?? []);
  const { data: sessionsData } = useSessions(agent);
  const sessionRunStatus = sessionsData?.sessions?.find((s: { id: string }) => s.id === activeSessionId)?.run_status;

  return !!activeSessionId && (
    isActivePhase(connectionPhase) ||
    activeSessionIds.includes(activeSessionId) ||
    sessionRunStatus === "running"
  );
}

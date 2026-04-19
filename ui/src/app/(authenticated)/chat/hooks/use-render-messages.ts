"use client";

import { useMemo } from "react";
import { useChatStore } from "@/stores/chat-store";
import { selectRenderMessages } from "@/stores/chat-selectors";
import type { ChatMessage } from "@/stores/chat-types";

/**
 * Subscribes to the underlying stable fields (not the derived array)
 * and memoizes the result. `selectRenderMessages` creates a fresh
 * array on every call (`[]` / `mergeLiveOverlay(...)`), so passing it
 * to `useChatStore` directly as a selector causes an infinite render
 * loop: Zustand's `Object.is` comparison of the returned reference
 * against the previous one is always false, triggering a re-render
 * that runs the selector again.
 *
 * Fix: subscribe only to the primitive/object inputs whose identity
 * is stable when unchanged (Immer preserves references on no-op
 * writes), and compute the derived array through `useMemo`. The
 * memo's dependency list changes only when the actual inputs do.
 */
export function useRenderMessages(agent: string): ChatMessage[] {
  const messageSource = useChatStore((s) => s.agents[agent]?.messageSource);
  const selectedBranches = useChatStore((s) => s.agents[agent]?.selectedBranches);
  const activeSessionId = useChatStore((s) => s.agents[agent]?.activeSessionId ?? null);

  return useMemo(() => {
    // Guard: agent slot not yet initialised — return stable empty array.
    if (!messageSource) return [];

    // Rebuild the selector's inputs into a minimal fake state so we
    // can reuse its logic without duplication. The selector only
    // reads these three fields from `state.agents[agent]`.
    const fakeState = {
      agents: {
        [agent]: {
          messageSource,
          selectedBranches,
          activeSessionId,
        },
      },
    } as any;
    return selectRenderMessages(fakeState, agent);
    // messageSource, selectedBranches, activeSessionId are the only
    // inputs that can influence the result. All three have stable
    // identity across renders when their values do not change
    // (Immer draft).
  }, [messageSource, selectedBranches, activeSessionId, agent]);
}

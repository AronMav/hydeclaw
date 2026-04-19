"use client";

import { useChatStore } from "@/stores/chat-store";
import { selectIsReplayingHistory } from "@/stores/chat-selectors";

export function useIsReplayingHistory(agent: string): boolean {
  return useChatStore((s) => selectIsReplayingHistory(s, agent));
}

"use client";

import { useChatStore } from "@/stores/chat-store";
import { selectIsLive } from "@/stores/chat-selectors";

export function useIsLive(agent: string): boolean {
  return useChatStore((s) => selectIsLive(s, agent));
}

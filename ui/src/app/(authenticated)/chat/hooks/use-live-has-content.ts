"use client";

import { useChatStore } from "@/stores/chat-store";
import { selectLiveHasContent } from "@/stores/chat-selectors";

export function useLiveHasContent(agent: string): boolean {
  return useChatStore((s) => selectLiveHasContent(s, agent));
}

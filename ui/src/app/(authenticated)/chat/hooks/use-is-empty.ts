"use client";

import { useChatStore } from "@/stores/chat-store";
import { selectIsEmpty } from "@/stores/chat-selectors";

export function useIsEmpty(agent: string): boolean {
  return useChatStore((s) => selectIsEmpty(s, agent));
}

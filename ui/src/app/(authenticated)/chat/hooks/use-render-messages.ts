"use client";

import { useChatStore } from "@/stores/chat-store";
import { selectRenderMessages } from "@/stores/chat-selectors";
import type { ChatMessage } from "@/stores/chat-types";

export function useRenderMessages(agent: string): ChatMessage[] {
  return useChatStore((s) => selectRenderMessages(s, agent));
}

"use client";

import type { ReactNode } from "react";

/**
 * Passthrough provider -- assistant-ui dependency removed in Phase 13.
 * Retained for API stability (page.tsx imports ChatRuntimeProvider).
 * All data flows through Zustand (chat-store) and React Query hooks.
 */
export function ChatRuntimeProvider({ children }: { children: ReactNode }) {
  return <>{children}</>;
}

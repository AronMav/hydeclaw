/**
 * Chat reconciliation utilities.
 *
 * OPTI-03: Content hash reconciliation for detecting live/history message diffs.
 */

import type { ChatMessage } from "./chat-types";

/**
 * Fast djb2-style hash of a ChatMessage array.
 * Compares id + role + text content of parts -- intentionally ignores createdAt
 * (timestamps differ between live SSE messages and DB history rows).
 * Used for render-optimization, not security.
 */
export function contentHash(messages: ChatMessage[]): string {
  let hash = 0;
  for (const m of messages) {
    const str =
      m.id +
      m.role +
      m.parts
        .map((p) =>
          p.type === "text"
            ? p.text
            : p.type + ("toolCallId" in p ? (p as { toolCallId: string }).toolCallId : ""),
        )
        .join("|");
    for (let i = 0; i < str.length; i++) {
      hash = ((hash << 5) - hash + str.charCodeAt(i)) | 0;
    }
  }
  return hash.toString(36);
}

/**
 * OPTI-03: Compare live messages with freshly-fetched history.
 * Returns null if content is identical (skip re-render), or returns history
 * messages when they differ (history has extra data or server post-processing).
 */
export function reconcileLiveWithHistory(
  live: ChatMessage[],
  history: ChatMessage[],
): ChatMessage[] | null {
  if (live.length === history.length && contentHash(live) === contentHash(history)) {
    return null; // identical -- skip re-render
  }
  return history;
}

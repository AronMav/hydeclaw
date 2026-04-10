import type { ChatMessage, MessagePart } from "./chat-types";
import type { MessageRow } from "@/types/api";
import { parseContentParts } from "@/stores/sse-events";
import { queryClient } from "@/lib/query-client";
import { qk } from "@/lib/queries";

// ── History conversion (MessageRow[] -> ChatMessage[]) ───────────────────────

/**
 * Converts flat database rows into structured ChatMessage objects.
 * Implements "Virtual Merging" (Stage 2): consecutive assistant/tool blocks
 * from the same agent are merged into a single visual message to ensure
 * stable tool grouping and consistent identity.
 */
export function convertHistory(rows: MessageRow[], isAgentStreaming?: boolean, selectedBranches?: Record<string, string>): ChatMessage[] {
  // When branching data exists and selectedBranches provided, resolve active path first
  const resolvedRows = selectedBranches && rows.some(r => r.parent_message_id != null)
    ? resolveActivePath(rows, selectedBranches)
    : rows;
  // Filter out streaming placeholder messages:
  // - Always filter if we have an active live stream (stream provides the content)
  // - Also filter empty streaming rows in history mode (ghost rows from interrupted streams)
  const filtered = resolvedRows.filter(m => {
    if (m.status === "streaming" && isAgentStreaming) return false;
    if (m.status === "streaming" && !m.content && !m.tool_calls) return false;
    return true;
  });

  const messages: ChatMessage[] = [];
  let lastAssistantMsg: ChatMessage | null = null;
  let lastAgentId: string | undefined = undefined;

  // Tool call map for resolving tool names/inputs from the main assistant record
  const toolCallMap = new Map<string, { name: string; arguments: unknown }>();
  for (const m of filtered) {
    if (m.role === "assistant" && m.tool_calls) {
      const calls = m.tool_calls as Array<{ id: string; name: string; arguments?: unknown }>;
      if (Array.isArray(calls)) {
        for (const tc of calls) {
          if (tc.id) toolCallMap.set(tc.id, { name: tc.name || "tool", arguments: tc.arguments ?? {} });
        }
      }
    }
  }

  for (const m of filtered) {
    if (m.role === "user") {
      // Finalize any pending assistant message before starting a user block
      if (lastAssistantMsg) {
        messages.push(lastAssistantMsg);
        lastAssistantMsg = null;
      }
      if (m.agent_id) lastAgentId = m.agent_id;
      messages.push({
        id: m.id,
        role: "user",
        parts: [{ type: "text", text: m.content || "" }],
        createdAt: m.created_at,
        agentId: m.agent_id ?? undefined,
        parentMessageId: m.parent_message_id ?? undefined,
        branchFromMessageId: m.branch_from_message_id ?? undefined,
      });
    } else if (m.role === "assistant" && !m.tool_call_id) {
      // Assistant text block
      const assistantAgentId = m.agent_id ?? lastAgentId;
      if (m.agent_id) lastAgentId = m.agent_id;

      const newParts = parseContentParts(m.content || "");

      // D-01: No merging. Each assistant DB row becomes its own ChatMessage.
      // Virtual Merging was removed because it breaks tool call ordering —
      // tools must appear between the assistant messages that invoked them.
      if (lastAssistantMsg) messages.push(lastAssistantMsg);
      lastAssistantMsg = {
        id: m.id,
        role: "assistant",
        parts: newParts,
        createdAt: m.created_at,
        agentId: assistantAgentId,
        parentMessageId: m.parent_message_id ?? undefined,
        branchFromMessageId: m.branch_from_message_id ?? undefined,
      };
    } else if (m.role === "tool" && m.tool_call_id) {
      // Tool result block — always attach to the latest assistant message
      if (lastAssistantMsg) {
        const tc = toolCallMap.get(m.tool_call_id);

        // Extract inline files (__file__: markers)
        const lines = (m.content || "").split("\n");
        const cleanLines: string[] = [];
        for (const line of lines) {
          if (line.startsWith("__file__:")) {
            try {
              const meta = JSON.parse(line.slice("__file__:".length));
              if (meta.url) {
                lastAssistantMsg.parts.push({
                  type: "file",
                  url: meta.url,
                  mediaType: meta.mediaType || "image/png",
                });
              }
            } catch { /* ignore */ }
          } else {
            cleanLines.push(line);
          }
        }

        lastAssistantMsg.parts.push({
          type: "tool",
          toolCallId: m.tool_call_id,
          toolName: tc?.name || "tool",
          state: "output-available",
          input: (tc?.arguments as Record<string, unknown>) ?? {},
          output: cleanLines.join("\n"),
        });
      }
    }
  }

  if (lastAssistantMsg) messages.push(lastAssistantMsg);

  // Final pass: filter empty messages and stabilize referential identity
  return messages.filter(m => m.parts.length > 0);
}

/**
 * Read-through cache peek — called from Zustand store actions where React hooks
 * are unavailable. Components access this data via useSessionMessages() hook.
 * See ARCH-02 audit (phase 34): queryClient.getQueryData is intentional here and
 * in sendMessage(); no React component calls getQueryData directly.
 */
export function getCachedHistoryMessages(sessionId: string | null, selectedBranches?: Record<string, string>): ChatMessage[] {
  if (!sessionId) return [];
  const cached = queryClient.getQueryData<{ messages: MessageRow[] }>(qk.sessionMessages(sessionId));
  return cached ? convertHistory(cached.messages, false, selectedBranches) : [];
}

/** Get all raw MessageRow[] from React Query cache for a session (for sibling discovery). */
export function getCachedRawMessages(sessionId: string | null): MessageRow[] {
  if (!sessionId) return [];
  const cached = queryClient.getQueryData<{ messages: MessageRow[] }>(qk.sessionMessages(sessionId));
  return cached?.messages ?? [];
}

// ── Branch resolution ─────────────────────────────────────────────────────

/**
 * Given all messages (including all branches) and the user's branch selections,
 * returns the linear path of messages to display.
 */
export function resolveActivePath(
  rows: MessageRow[],
  selectedBranches: Record<string, string>,
): MessageRow[] {
  const hasBranching = rows.some(r => r.parent_message_id != null);
  if (!hasBranching) {
    return [...rows].sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
  }

  const childrenOf = new Map<string, MessageRow[]>();
  const roots: MessageRow[] = [];

  for (const r of rows) {
    if (r.parent_message_id == null) {
      roots.push(r);
    } else {
      const siblings = childrenOf.get(r.parent_message_id) ?? [];
      siblings.push(r);
      childrenOf.set(r.parent_message_id, siblings);
    }
  }

  for (const [, children] of childrenOf) {
    children.sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
  }

  roots.sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
  if (roots.length === 0) return [];

  const path: MessageRow[] = [];
  let current: MessageRow | undefined = roots[0];

  while (current) {
    path.push(current);
    const children = childrenOf.get(current.id);
    if (!children || children.length === 0) break;

    const selectedId: string | undefined = selectedBranches[current.id];
    current = selectedId
      ? children.find(c => c.id === selectedId) ?? children[children.length - 1]
      : children[children.length - 1];
  }

  return path;
}

/** Find all sibling messages (sharing the same parent, same role). */
export function findSiblings(rows: MessageRow[], messageId: string): { siblings: MessageRow[]; index: number } {
  const msg = rows.find(r => r.id === messageId);
  if (!msg || !msg.parent_message_id) return { siblings: msg ? [msg] : [], index: 0 };

  const siblings = rows
    .filter(r => r.parent_message_id === msg.parent_message_id && r.role === msg.role)
    .sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());

  return { siblings, index: siblings.findIndex(s => s.id === messageId) };
}

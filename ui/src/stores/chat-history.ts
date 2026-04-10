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
export function convertHistory(rows: MessageRow[], isAgentStreaming?: boolean): ChatMessage[] {
  // Filter out streaming placeholder messages ONLY if we have an active live stream
  // that will provide the same content. If not, show them as fallback (history).
  const filtered = rows.filter(m => {
    if (m.status === "streaming" && isAgentStreaming) return false;
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
export function getCachedHistoryMessages(sessionId: string | null): ChatMessage[] {
  if (!sessionId) return [];
  const cached = queryClient.getQueryData<{ messages: MessageRow[] }>(qk.sessionMessages(sessionId));
  return cached ? convertHistory(cached.messages) : [];
}

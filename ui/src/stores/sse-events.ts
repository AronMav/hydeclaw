// Discriminated union of all SSE events from the backend.
// Keep in sync with enum StreamEvent in crates/hydeclaw-core/src/agent/engine.rs.

export interface AgentTurnCard {
  agentName: string;
  reason: string;
}

export type SseEvent =
  | { type: "data-session-id"; data: { sessionId: string } }
  | { type: "start"; messageId?: string; agentName?: string }
  | { type: "text-start"; id?: string; agentName?: string }
  | { type: "text-delta"; delta: string }
  | { type: "text-end" }
  | { type: "tool-input-start"; toolCallId: string; toolName: string; agentName?: string }
  | { type: "tool-input-delta"; toolCallId: string; inputTextDelta: string }
  | { type: "tool-input-available"; toolCallId: string; input: unknown }
  | { type: "tool-output-available"; toolCallId: string; output: unknown }
  | { type: "file"; url: string; mediaType?: string }
  | { type: "rich-card"; cardType: "table" | "metric" | "agent-turn"; data: Record<string, unknown> }
  | { type: "sync"; content: string; toolCalls: unknown[]; status: string; error?: string }
  | { type: "finish"; agentName?: string }
  | { type: "error"; errorText: string };

/**
 * Parse and validate a single SSE data payload.
 * Returns null for invalid JSON, missing type, or unknown event type.
 */
export function parseSseEvent(raw: string): SseEvent | null {
  let obj: unknown;
  try {
    obj = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!obj || typeof obj !== "object") return null;
  const e = obj as Record<string, unknown>;
  const type = e.type;
  if (typeof type !== "string") return null;

  switch (type) {
    case "data-session-id": {
      const data = e.data as Record<string, unknown> | undefined;
      if (!data || typeof data.sessionId !== "string") return null;
      return { type, data: { sessionId: data.sessionId } };
    }
    case "start":
      return { type, messageId: typeof e.messageId === "string" ? e.messageId : undefined, agentName: typeof e.agentName === "string" ? e.agentName : undefined };
    case "text-start":
      return { type, id: typeof e.id === "string" ? e.id : undefined, agentName: typeof e.agentName === "string" ? e.agentName : undefined };
    case "text-delta":
      return { type, delta: typeof e.delta === "string" ? e.delta : "" };
    case "text-end":
      return { type };
    case "tool-input-start":
      if (typeof e.toolCallId !== "string" || typeof e.toolName !== "string") return null;
      return { type, toolCallId: e.toolCallId, toolName: e.toolName, agentName: typeof e.agentName === "string" ? e.agentName : undefined };
    case "tool-input-delta":
      if (typeof e.toolCallId !== "string") return null;
      return { type, toolCallId: e.toolCallId, inputTextDelta: typeof e.inputTextDelta === "string" ? e.inputTextDelta : "" };
    case "tool-input-available":
      if (typeof e.toolCallId !== "string") return null;
      return { type, toolCallId: e.toolCallId, input: e.input ?? {} };
    case "tool-output-available":
      if (typeof e.toolCallId !== "string") return null;
      return { type, toolCallId: e.toolCallId, output: e.output };
    case "file":
      if (typeof e.url !== "string") return null;
      return { type, url: e.url, mediaType: typeof e.mediaType === "string" ? e.mediaType : undefined };
    case "rich-card":
      return { type, cardType: e.cardType as "table" | "metric" | "agent-turn", data: (e.data as Record<string, unknown>) ?? {} };
    case "sync":
      return {
        type,
        content: typeof e.content === "string" ? e.content : "",
        toolCalls: Array.isArray(e.toolCalls) ? e.toolCalls : [],
        status: typeof e.status === "string" ? e.status : "unknown",
        error: typeof e.error === "string" ? e.error : undefined,
      };
    case "finish":
      return { type, agentName: typeof e.agentName === "string" ? e.agentName : undefined };
    case "error":
      return { type, errorText: typeof e.errorText === "string" ? e.errorText : "Unknown error" };
    default:
      return null;
  }
}

/**
 * Splits an incoming chunk into complete SSE lines, buffering incomplete ones.
 * Extracted from chat-store.ts to be testable and reusable.
 */
export function parseSSELines(chunk: string, buffer: { current: string }): string[] {
  buffer.current += chunk;
  const lines: string[] = [];
  let idx: number;
  while ((idx = buffer.current.indexOf("\n")) !== -1) {
    lines.push(buffer.current.slice(0, idx).replace(/\r$/, ""));
    buffer.current = buffer.current.slice(idx + 1);
  }
  return lines;
}

// ── Types for parseContentParts (defined here to avoid circular import with chat-store) ──
interface TextPart { type: "text"; text: string; }
interface ReasoningPart { type: "reasoning"; text: string; }
type ParsedPart = TextPart | ReasoningPart;

/** Extract <think> blocks into reasoning parts and clean text parts from raw content */
export function parseContentParts(raw: string): ParsedPart[] {
  if (!raw) return [];
  const parts: ParsedPart[] = [];
  const thinkRegex = /<think>([\s\S]*?)<\/think>/g;
  let lastIndex = 0;
  let match;

  while ((match = thinkRegex.exec(raw)) !== null) {
    const before = raw.slice(lastIndex, match.index).trim();
    if (before) parts.push({ type: "text", text: before });
    const reasoning = match[1].trim();
    if (reasoning) parts.push({ type: "reasoning", text: reasoning });
    lastIndex = match.index + match[0].length;
  }

  // Handle remaining text after last closed </think> (or all text if no <think> blocks)
  let after = raw.slice(lastIndex).trim();
  // Handle unclosed <think> at end of remaining text
  const unclosedIdx = after.indexOf("<think>");
  if (unclosedIdx >= 0) {
    const beforeUnclosed = after.slice(0, unclosedIdx).trim();
    if (beforeUnclosed) {
      const cleanedBefore = beforeUnclosed
        .replace(/<minimax:tool_call>[\s\S]*?(<\/minimax:tool_call>|$)\s*/g, "")
        .replace(/\[TOOL_CALL\][\s\S]*?(\[\/TOOL_CALL\]|$)\s*/g, "")
        .trim();
      if (cleanedBefore) parts.push({ type: "text", text: cleanedBefore });
    }
    const unclosedReasoning = after.slice(unclosedIdx + 7).trim();
    if (unclosedReasoning) parts.push({ type: "reasoning", text: unclosedReasoning });
  } else {
    const cleaned = after
      .replace(/<minimax:tool_call>[\s\S]*?(<\/minimax:tool_call>|$)\s*/g, "")
      .replace(/\[TOOL_CALL\][\s\S]*?(\[\/TOOL_CALL\]|$)\s*/g, "")
      .trim();
    if (cleaned) parts.push({ type: "text", text: cleaned });
  }

  return parts;
}

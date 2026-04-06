import { vi, describe, it, expect } from "vitest";

// Mock dependencies before importing chat-store
vi.mock("@/lib/query-client", () => ({
  queryClient: { invalidateQueries: vi.fn(), getQueryData: vi.fn(() => undefined) },
}));
vi.mock("@/lib/api", () => ({
  apiGet: vi.fn(),
  apiDelete: vi.fn(),
  apiPatch: vi.fn(),
  getToken: vi.fn(() => "test-token"),
}));

import {
  isActiveStream,
  convertHistory,
  MAX_INPUT_LENGTH,
  getInitialAgent,
  getLastSessionId,
  saveLastSession,
} from "@/stores/chat-store";
import type { MessageRow } from "@/types/api";

// ── Constants ───────────────────────────────────────────────────────────────

describe("MAX_INPUT_LENGTH", () => {
  it("is 32000", () => {
    expect(MAX_INPUT_LENGTH).toBe(32_000);
  });
});

// ── convertHistory ──────────────────────────────────────────────────────────

function makeRow(overrides: Partial<MessageRow>): MessageRow {
  return {
    id: "m1",
    role: "user",
    content: "",
    tool_calls: null,
    tool_call_id: null,
    created_at: "2026-01-01T00:00:00Z",
    status: "done",
    feedback: 0,
    edited_at: null,
    ...overrides,
  };
}

describe("convertHistory", () => {
  it("converts a simple user+assistant exchange", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "Hello" }),
      makeRow({ id: "a1", role: "assistant", content: "Hi there" }),
    ];
    const msgs = convertHistory(rows);
    expect(msgs).toHaveLength(2);
    expect(msgs[0].role).toBe("user");
    expect(msgs[0].parts).toEqual([{ type: "text", text: "Hello" }]);
    expect(msgs[1].role).toBe("assistant");
    expect(msgs[1].parts[0]).toEqual({ type: "text", text: "Hi there" });
  });

  it("filters out streaming messages", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "Hi" }),
      makeRow({ id: "a1", role: "assistant", content: "partial...", status: "streaming" }),
      makeRow({ id: "a2", role: "assistant", content: "Full response", status: "complete" }),
    ];
    const msgs = convertHistory(rows);
    const assistantMsgs = msgs.filter(m => m.role === "assistant");
    expect(assistantMsgs).toHaveLength(1);
    expect(assistantMsgs[0].parts[0]).toEqual({ type: "text", text: "Full response" });
  });

  it("extracts <think> blocks as reasoning parts", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "question" }),
      makeRow({
        id: "a1",
        role: "assistant",
        content: "<think>Let me think...</think>The answer is 42.",
      }),
    ];
    const msgs = convertHistory(rows);
    const parts = msgs[1].parts;
    expect(parts).toHaveLength(2);
    expect(parts[0]).toEqual({ type: "reasoning", text: "Let me think..." });
    expect(parts[1]).toEqual({ type: "text", text: "The answer is 42." });
  });

  it("handles tool call lifecycle (assistant+tool rows)", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "search for cats" }),
      makeRow({
        id: "a1",
        role: "assistant",
        content: "",
        tool_calls: [{ id: "tc1", name: "search", arguments: { q: "cats" } }],
      }),
      makeRow({
        id: "t1",
        role: "tool",
        content: "Found 5 results",
        tool_call_id: "tc1",
      }),
      makeRow({ id: "a2", role: "assistant", content: "Here are your results." }),
    ];
    const msgs = convertHistory(rows);
    // Should have: user, assistant (with tool part), assistant (text)
    expect(msgs.length).toBeGreaterThanOrEqual(2);
    const toolPart = msgs.flatMap(m => m.parts).find(p => p.type === "tool");
    expect(toolPart?.type).toBe("tool");
    if (toolPart?.type === "tool") {
      expect(toolPart.toolName).toBe("search");
      expect(toolPart.state).toBe("output-available");
      expect(toolPart.output).toBe("Found 5 results");
    }
  });

  it("extracts __file__ markers from tool output", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "show image" }),
      makeRow({
        id: "a1",
        role: "assistant",
        content: "",
        tool_calls: [{ id: "tc1", name: "img", arguments: {} }],
      }),
      makeRow({
        id: "t1",
        role: "tool",
        content: '__file__:{"url":"/img.png","mediaType":"image/png"}\nDone',
        tool_call_id: "tc1",
      }),
    ];
    const msgs = convertHistory(rows);
    const parts = msgs.flatMap(m => m.parts);
    const filePart = parts.find(p => p.type === "file");
    expect(filePart).toEqual({ type: "file", url: "/img.png", mediaType: "image/png" });
    const toolPart = parts.find(p => p.type === "tool");
    if (toolPart?.type === "tool") {
      expect(toolPart.output).toBe("Done");
    }
  });

  it("returns empty array for empty input", () => {
    expect(convertHistory([])).toEqual([]);
  });

  it("preserves agentId from rows", () => {
    const rows: MessageRow[] = [
      makeRow({ id: "u1", role: "user", content: "hi", agent_id: "Agent1" }),
    ];
    const msgs = convertHistory(rows);
    expect(msgs[0].agentId).toBe("Agent1");
  });
});

// ── localStorage helpers ────────────────────────────────────────────────────

describe("getInitialAgent", () => {
  it("returns first agent when nothing saved", () => {
    localStorage.removeItem("hydeclaw.lastSession");
    expect(getInitialAgent(["A", "B"])).toBe("A");
  });

  it("returns empty string for empty list", () => {
    expect(getInitialAgent([])).toBe("");
  });
});

describe("saveLastSession / getLastSessionId", () => {
  it("saves and retrieves session id per agent", () => {
    saveLastSession("Agent1", "sess-1");
    expect(getLastSessionId("Agent1")).toBe("sess-1");
  });

  it("returns undefined for unknown agent", () => {
    localStorage.removeItem("hydeclaw.lastSession");
    expect(getLastSessionId("Unknown")).toBeUndefined();
  });
});

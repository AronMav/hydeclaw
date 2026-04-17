import { describe, it, expect } from "vitest";
import { mergeLiveOverlay } from "@/stores/chat-overlay-dedup";
import type { ChatMessage } from "@/stores/chat-types";

// ── Helpers ─────────────────────────────────────────────────────────────────

function userMsg(
  id: string,
  text: string,
  status?: "sending" | "confirmed" | "failed",
): ChatMessage {
  return {
    id,
    role: "user",
    parts: [{ type: "text", text }],
    createdAt: new Date().toISOString(),
    status,
  };
}

function assistantMsg(id: string, text: string): ChatMessage {
  return {
    id,
    role: "assistant",
    parts: [{ type: "text", text }],
    createdAt: new Date().toISOString(),
  };
}

// ── Regression: 2026-04-17 "sent message disappears" ───────────────────────

describe("mergeLiveOverlay — user bubble visibility", () => {
  it("shows a SENDING user bubble when history is empty (fresh send)", () => {
    const history: ChatMessage[] = [];
    const live: ChatMessage[] = [userMsg("u1", "Hello agent", "sending")];
    const out = mergeLiveOverlay(history, live);
    expect(out).toHaveLength(1);
    expect(out[0].role).toBe("user");
    expect(out[0].id).toBe("u1");
  });

  it("STILL shows a CONFIRMED user bubble until history mirrors it (regression)", () => {
    // Before the fix, status === "confirmed" caused `continue` that dropped
    // the optimistic bubble, leaving chat empty while the agent worked.
    const history: ChatMessage[] = [];
    const live: ChatMessage[] = [userMsg("u1", "Hello agent", "confirmed")];
    const out = mergeLiveOverlay(history, live);
    expect(out).toHaveLength(1);
    expect(out[0].role).toBe("user");
    expect(out[0].status).toBe("confirmed");
  });

  it("STILL shows a FAILED user bubble until rollback UI replaces it", () => {
    const history: ChatMessage[] = [];
    const live: ChatMessage[] = [userMsg("u1", "bad message", "failed")];
    const out = mergeLiveOverlay(history, live);
    expect(out).toHaveLength(1);
    expect(out[0].status).toBe("failed");
  });

  it("DEDUPS when history already contains the same user text", () => {
    const history: ChatMessage[] = [
      userMsg("db-1", "Hello agent"),
      assistantMsg("db-2", "Hi!"),
    ];
    const live: ChatMessage[] = [userMsg("u1", "Hello agent", "confirmed")];
    const out = mergeLiveOverlay(history, live);
    // History has 2, live 1 — dedup removes live copy → still 2.
    expect(out).toHaveLength(2);
    expect(out[0].id).toBe("db-1"); // history user survives
    // No "u1" in the output.
    expect(out.every((m) => m.id !== "u1")).toBe(true);
  });

  it("shows BOTH history and live when texts differ (second send)", () => {
    const history: ChatMessage[] = [userMsg("db-1", "First")];
    const live: ChatMessage[] = [userMsg("u1", "Second", "sending")];
    const out = mergeLiveOverlay(history, live);
    expect(out).toHaveLength(2);
    expect(out[0].id).toBe("db-1");
    expect(out[1].id).toBe("u1");
  });
});

describe("mergeLiveOverlay — assistant dedup", () => {
  it("drops empty assistant placeholders", () => {
    const history: ChatMessage[] = [];
    const live: ChatMessage[] = [
      {
        id: "a1",
        role: "assistant",
        parts: [],
        createdAt: new Date().toISOString(),
      },
    ];
    expect(mergeLiveOverlay(history, live)).toEqual([]);
  });

  it("strips text parts already in history by first-80-char fingerprint", () => {
    const history: ChatMessage[] = [assistantMsg("db-a", "Hello world")];
    const live: ChatMessage[] = [assistantMsg("live-a", "Hello world")];
    const out = mergeLiveOverlay(history, live);
    // Live copy's only part is a dup → entire message filtered out.
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe("db-a");
  });

  it("strips tool parts already in history by toolCallId", () => {
    const historyWithTool: ChatMessage = {
      id: "db-a",
      role: "assistant",
      parts: [{ type: "tool", toolCallId: "tc1", toolName: "search", state: "output-available", input: {}, output: "" }],
      createdAt: new Date().toISOString(),
    };
    const liveWithSameTool: ChatMessage = {
      id: "live-a",
      role: "assistant",
      parts: [{ type: "tool", toolCallId: "tc1", toolName: "search", state: "output-available", input: {}, output: "" }],
      createdAt: new Date().toISOString(),
    };
    const out = mergeLiveOverlay([historyWithTool], [liveWithSameTool]);
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe("db-a");
  });
});

describe("mergeLiveOverlay — edge cases", () => {
  it("returns history unchanged when live is empty", () => {
    const history: ChatMessage[] = [assistantMsg("a1", "hi")];
    expect(mergeLiveOverlay(history, [])).toBe(history);
  });

  it("returns history unchanged when live overlay has only duplicates", () => {
    const history: ChatMessage[] = [
      userMsg("db-u", "Hello"),
      assistantMsg("db-a", "Hi there"),
    ];
    const live: ChatMessage[] = [
      userMsg("live-u", "Hello", "confirmed"),
      assistantMsg("live-a", "Hi there"),
    ];
    const out = mergeLiveOverlay(history, live);
    // All live items are duplicates → history returned verbatim.
    expect(out).toEqual(history);
  });
});

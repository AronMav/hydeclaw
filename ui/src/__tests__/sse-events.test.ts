import { describe, it, expect } from "vitest";
import { parseSseEvent, parseSSELines } from "@/stores/sse-events";

describe("parseSseEvent — full coverage", () => {
  it("parses data-session-id event", () => {
    const e = parseSseEvent(JSON.stringify({
      type: "data-session-id",
      data: { sessionId: "sess-abc" },
    }));
    expect(e).toEqual({ type: "data-session-id", data: { sessionId: "sess-abc" } });
  });

  it("returns null for data-session-id with missing sessionId", () => {
    expect(parseSseEvent(JSON.stringify({ type: "data-session-id", data: {} }))).toBeNull();
    expect(parseSseEvent(JSON.stringify({ type: "data-session-id" }))).toBeNull();
  });

  it("parses start event with optional messageId", () => {
    expect(parseSseEvent(JSON.stringify({ type: "start", messageId: "m1" }))).toEqual({
      type: "start",
      messageId: "m1",
    });
    expect(parseSseEvent(JSON.stringify({ type: "start" }))).toEqual({
      type: "start",
      messageId: undefined,
    });
  });

  it("parses text-start with optional id", () => {
    expect(parseSseEvent(JSON.stringify({ type: "text-start", id: "t1" }))).toEqual({
      type: "text-start",
      id: "t1",
    });
    expect(parseSseEvent(JSON.stringify({ type: "text-start" }))).toEqual({
      type: "text-start",
      id: undefined,
    });
  });

  it("parses text-end event", () => {
    expect(parseSseEvent(JSON.stringify({ type: "text-end" }))).toEqual({ type: "text-end" });
  });

  it("parses tool-input-delta event", () => {
    const e = parseSseEvent(JSON.stringify({
      type: "tool-input-delta",
      toolCallId: "tc1",
      inputTextDelta: '{"q":',
    }));
    expect(e).toEqual({
      type: "tool-input-delta",
      toolCallId: "tc1",
      inputTextDelta: '{"q":',
    });
  });

  it("returns null for tool-input-delta without toolCallId", () => {
    expect(parseSseEvent(JSON.stringify({ type: "tool-input-delta" }))).toBeNull();
  });

  it("parses tool-input-available event", () => {
    const e = parseSseEvent(JSON.stringify({
      type: "tool-input-available",
      toolCallId: "tc1",
      input: { query: "test" },
    }));
    expect(e).toEqual({
      type: "tool-input-available",
      toolCallId: "tc1",
      input: { query: "test" },
    });
  });

  it("defaults tool-input-available input to empty object", () => {
    const e = parseSseEvent(JSON.stringify({
      type: "tool-input-available",
      toolCallId: "tc1",
    }));
    expect(e?.type === "tool-input-available" && e.input).toEqual({});
  });

  it("parses file event", () => {
    const e = parseSseEvent(JSON.stringify({
      type: "file",
      url: "/img.png",
      mediaType: "image/png",
    }));
    expect(e).toEqual({ type: "file", url: "/img.png", mediaType: "image/png" });
  });

  it("returns null for file without url", () => {
    expect(parseSseEvent(JSON.stringify({ type: "file" }))).toBeNull();
  });

  it("parses file event without mediaType", () => {
    const e = parseSseEvent(JSON.stringify({ type: "file", url: "/f.bin" }));
    expect(e).toEqual({ type: "file", url: "/f.bin", mediaType: undefined });
  });

  it("parses rich-card event", () => {
    const e = parseSseEvent(JSON.stringify({
      type: "rich-card",
      cardType: "table",
      data: { rows: [] },
    }));
    expect(e).toEqual({ type: "rich-card", cardType: "table", data: { rows: [] } });
  });

  it("parses sync event", () => {
    const e = parseSseEvent(JSON.stringify({
      type: "sync",
      content: "hello",
      toolCalls: [{ id: "tc1" }],
      status: "complete",
      error: "oops",
    }));
    expect(e).toEqual({
      type: "sync",
      content: "hello",
      toolCalls: [{ id: "tc1" }],
      status: "complete",
      error: "oops",
    });
  });

  it("defaults sync fields when missing", () => {
    const e = parseSseEvent(JSON.stringify({ type: "sync" }));
    expect(e).toEqual({
      type: "sync",
      content: "",
      toolCalls: [],
      status: "unknown",
      error: undefined,
    });
  });

  it("parses error event with default errorText", () => {
    const e = parseSseEvent(JSON.stringify({ type: "error" }));
    expect(e).toEqual({ type: "error", errorText: "Unknown error" });
  });

  it("returns null for non-object JSON", () => {
    expect(parseSseEvent('"just a string"')).toBeNull();
    expect(parseSseEvent("42")).toBeNull();
    expect(parseSseEvent("null")).toBeNull();
  });

  it("returns null for non-string type field", () => {
    expect(parseSseEvent(JSON.stringify({ type: 123 }))).toBeNull();
  });
});

describe("parseSSELines — edge cases", () => {
  it("handles empty chunk", () => {
    const buf = { current: "" };
    expect(parseSSELines("", buf)).toEqual([]);
    expect(buf.current).toBe("");
  });

  it("handles multiple newlines producing empty lines", () => {
    const buf = { current: "" };
    const lines = parseSSELines("\n\n", buf);
    expect(lines).toEqual(["", ""]);
  });

  it("accumulates across multiple calls", () => {
    const buf = { current: "" };
    parseSSELines("data: par", buf);
    parseSSELines("tial\ndata: ", buf);
    const lines = parseSSELines("complete\n", buf);
    expect(lines).toEqual(["data: complete"]);
  });
});

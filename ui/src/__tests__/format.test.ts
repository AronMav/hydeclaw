import { describe, it, expect } from "vitest";
import { cleanContent, formatDuration, formatBytes } from "@/lib/format";

describe("cleanContent", () => {
  it("removes closed <think> blocks", () => {
    expect(cleanContent("Hello <think>reasoning</think> world")).toBe("Hello world");
  });

  it("removes unclosed <think> at end", () => {
    expect(cleanContent("Hello <think>still thinking")).toBe("Hello");
  });

  it("removes multiple <think> blocks", () => {
    expect(cleanContent("<think>a</think>text<think>b</think>more")).toBe("textmore");
  });

  it("removes minimax tool_call tags", () => {
    expect(cleanContent("Hello <minimax:tool_call>call</minimax:tool_call> world")).toBe("Hello world");
  });

  it("removes [TOOL_CALL] tags", () => {
    expect(cleanContent("Hello [TOOL_CALL]call[/TOOL_CALL] world")).toBe("Hello world");
  });

  it("returns empty for think-only content", () => {
    expect(cleanContent("<think>only reasoning</think>")).toBe("");
  });

  it("handles empty string", () => {
    expect(cleanContent("")).toBe("");
  });

  it("passes through normal text unchanged", () => {
    expect(cleanContent("Hello world")).toBe("Hello world");
  });
});

describe("formatDuration", () => {
  it("formats seconds", () => {
    expect(formatDuration(45)).toBe("45s");
  });

  it("formats minutes", () => {
    expect(formatDuration(120)).toBe("2m");
  });

  it("formats hours and minutes", () => {
    expect(formatDuration(3660)).toBe("1h 1m");
  });

  it("formats exact hours", () => {
    expect(formatDuration(7200)).toBe("2h");
  });

  it("handles zero", () => {
    expect(formatDuration(0)).toBe("0s");
  });
});

describe("formatBytes", () => {
  it("formats bytes", () => {
    expect(formatBytes(500)).toBe("500 B");
  });

  it("formats kilobytes", () => {
    expect(formatBytes(2048)).toBe("2.0 KB");
  });

  it("formats megabytes", () => {
    expect(formatBytes(5242880)).toBe("5.0 MB");
  });

  it("formats gigabytes", () => {
    expect(formatBytes(1073741824)).toBe("1.0 GB");
  });

  it("handles zero", () => {
    expect(formatBytes(0)).toBe("0 B");
  });
});

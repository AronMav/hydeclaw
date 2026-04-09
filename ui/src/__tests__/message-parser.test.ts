import { describe, it, expect } from "vitest";
import { IncrementalParser } from "@/lib/message-parser";

describe("IncrementalParser.reset()", () => {
  it("clears insideThink state — text after reset is classified as text, not reasoning", () => {
    const parser = new IncrementalParser();
    // Start a think block but don't close it — parser is now insideThink
    parser.processDelta("<think>partial reasoning");
    // Reset should clear insideThink
    parser.reset();
    // Now feed text — it should be plain text, not reasoning
    const parts = parser.processDelta("hello world text here that is long enough");
    const textParts = parts.filter(p => p.type === "text");
    const reasoningParts = parts.filter(p => p.type === "reasoning");
    expect(textParts.length).toBeGreaterThan(0);
    expect(reasoningParts.length).toBe(0);
  });

  it("clears accum — processDelta after reset returns empty parts for empty input", () => {
    const parser = new IncrementalParser();
    parser.processDelta("some buffered text");
    parser.reset();
    // After reset, accum is empty — empty delta should produce no new parts
    const parts = parser.processDelta("");
    expect(parts).toEqual([]);
  });

  it("clears parts — flush after reset returns empty", () => {
    const parser = new IncrementalParser();
    // Add substantial text to get something into parts
    parser.processDelta("some text that is long enough to exceed buffer and emit");
    // Reset should clear parts
    parser.reset();
    // Flush should return empty (no accumulated content)
    const flushed = parser.flush();
    expect(flushed).toEqual([]);
  });
});

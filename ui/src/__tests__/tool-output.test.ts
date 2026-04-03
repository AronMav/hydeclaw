import { describe, it, expect } from "vitest";
import { truncateOutput } from "@/lib/format";

describe("truncateOutput", () => {
  it("returns full text when under limit", () => {
    const result = truncateOutput("hello", 10_000);
    expect(result.truncated).toBe(false);
    expect(result.text).toBe("hello");
    expect(result.hiddenChars).toBe(0);
  });

  it("truncates text over limit", () => {
    const big = "x".repeat(15_000);
    const result = truncateOutput(big, 10_000);
    expect(result.truncated).toBe(true);
    expect(result.text.length).toBe(10_000);
    expect(result.hiddenChars).toBe(5_000);
  });

  it("reports hidden chars correctly", () => {
    const result = truncateOutput("a".repeat(12_345), 10_000);
    expect(result.hiddenChars).toBe(2_345);
  });

  it("text exactly at limit is not truncated", () => {
    const result = truncateOutput("a".repeat(10_000), 10_000);
    expect(result.truncated).toBe(false);
    expect(result.hiddenChars).toBe(0);
  });
});

import { describe, it, expect } from "vitest";
import { classifyStreamError } from "@/app/(authenticated)/chat/ChatThread";

describe("classifyStreamError", () => {
  it("Test 1: classifies 'Connection lost after retries' as connection_lost", () => {
    expect(classifyStreamError("Connection lost after retries")).toBe("connection_lost");
  });

  it("Test 2: classifies 'Failed to fetch' as connection_lost", () => {
    expect(classifyStreamError("Failed to fetch")).toBe("connection_lost");
  });

  it("Test 3: classifies 'LLM provider timeout' as timeout", () => {
    expect(classifyStreamError("LLM provider timeout")).toBe("timeout");
  });

  it("Test 4: classifies 'timeout' as timeout", () => {
    expect(classifyStreamError("timeout")).toBe("timeout");
  });

  it("Test 5: classifies 'HTTP 500: Internal Server Error' as api_error", () => {
    expect(classifyStreamError("HTTP 500: Internal Server Error")).toBe("api_error");
  });

  it("Test 6: classifies unknown error as api_error (default)", () => {
    expect(classifyStreamError("Some unknown error")).toBe("api_error");
  });

  it("Test 7: classifies 'Rate limited (429)' as api_error", () => {
    expect(classifyStreamError("Rate limited (429)")).toBe("api_error");
  });
});

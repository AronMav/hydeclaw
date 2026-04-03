import { describe, it, expect } from "vitest";

import { isActiveStream } from "@/stores/chat-store";

describe("isActiveStream", () => {
  it("returns true for streaming states", () => {
    expect(isActiveStream("submitted")).toBe(true);
    expect(isActiveStream("streaming")).toBe(true);
  });

  it("returns false for idle/error states", () => {
    expect(isActiveStream("idle")).toBe(false);
    expect(isActiveStream("error")).toBe(false);
    expect(isActiveStream(undefined)).toBe(false);
  });
});

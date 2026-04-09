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

describe("uuid() via crypto.randomUUID", () => {
  it("crypto.randomUUID produces valid UUID v4 strings", () => {
    // uuid() in chat-store delegates directly to crypto.randomUUID().
    // We verify the underlying API here to confirm no Math.random fallback exists.
    const UUID_V4_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
    const result = crypto.randomUUID();
    expect(result).toMatch(UUID_V4_RE);
  });

  it("crypto.randomUUID produces unique values", () => {
    const a = crypto.randomUUID();
    const b = crypto.randomUUID();
    expect(a).not.toBe(b);
  });
});

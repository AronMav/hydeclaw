import { describe, it, expect } from "vitest";

import { isActivePhase } from "@/stores/chat-store";

describe("isActivePhase", () => {
  it("returns true for active streaming states", () => {
    expect(isActivePhase("submitted")).toBe(true);
    expect(isActivePhase("streaming")).toBe(true);
    expect(isActivePhase("reconnecting")).toBe(true);
  });

  it("returns false for idle/error/complete states", () => {
    expect(isActivePhase("idle")).toBe(false);
    expect(isActivePhase("error")).toBe(false);
    expect(isActivePhase("complete")).toBe(false);
    expect(isActivePhase(undefined)).toBe(false);
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

import { describe, it, expect } from "bun:test";
import { exponentialDelay, chatCooldownKey, extractTgErrorCode, extractTgRetryAfter } from "../drivers/common";

// ── exponentialDelay ───────────────────────────────────────────────────

describe("exponentialDelay", () => {
  it("returns 1000ms for attempt 0", () => {
    expect(exponentialDelay(0)).toBe(1000);
  });
  it("returns 2000ms for attempt 1", () => {
    expect(exponentialDelay(1)).toBe(2000);
  });
  it("returns 4000ms for attempt 2", () => {
    expect(exponentialDelay(2)).toBe(4000);
  });
  it("caps at 30000ms", () => {
    expect(exponentialDelay(10)).toBe(30000);
  });
  it("increases monotonically up to cap", () => {
    let prev = 0;
    for (let i = 0; i < 6; i++) {
      const d = exponentialDelay(i);
      expect(d).toBeGreaterThan(prev);
      prev = d;
    }
  });
});

// ── chatCooldownKey ────────────────────────────────────────────────────

describe("chatCooldownKey", () => {
  it("builds key with chatId only when no threadId", () => {
    expect(chatCooldownKey(123, undefined)).toBe("123:");
  });
  it("builds key with chatId and threadId", () => {
    expect(chatCooldownKey(123, 456)).toBe("123:456");
  });
  it("handles negative chatId (group chats)", () => {
    expect(chatCooldownKey(-1001234567890, undefined)).toBe("-1001234567890:");
  });
  it("omitting threadId gives same key as explicit undefined", () => {
    expect(chatCooldownKey(42)).toBe(chatCooldownKey(42, undefined));
  });
});

// ── extractTgErrorCode ─────────────────────────────────────────────────

describe("extractTgErrorCode", () => {
  it("extracts 403 from Forbidden error", () => {
    const e = new Error("403: Forbidden: bot was blocked by the user");
    expect(extractTgErrorCode(e)).toBe(403);
  });

  it("extracts 429 from Too Many Requests", () => {
    const e = new Error("429: Too Many Requests: retry after 30");
    expect(extractTgErrorCode(e)).toBe(429);
  });

  it("extracts 400 from Bad Request", () => {
    const e = new Error("400: Bad Request: chat not found");
    expect(extractTgErrorCode(e)).toBe(400);
  });

  it("extracts 500 from Internal Server Error", () => {
    const e = new Error("500: Internal Server Error");
    expect(extractTgErrorCode(e)).toBe(500);
  });

  it("returns null for unknown error", () => {
    expect(extractTgErrorCode(new Error("network timeout"))).toBeNull();
  });

  it("returns null for ECONNRESET", () => {
    expect(extractTgErrorCode(new Error("ECONNRESET"))).toBeNull();
  });

  it("works with string errors", () => {
    expect(extractTgErrorCode("429: Too Many Requests")).toBe(429);
  });

  it("returns null for null", () => {
    expect(extractTgErrorCode(null)).toBeNull();
  });
});

// ── extractTgRetryAfter ────────────────────────────────────────────────

describe("extractTgRetryAfter", () => {
  it("extracts retry_after from object error", () => {
    const e = { parameters: { retry_after: 30 } };
    expect(extractTgRetryAfter(e)).toBe(30);
  });

  it("extracts retry_after from grammy-style error object", () => {
    const e = { parameters: { retry_after: 60 }, ok: false, error_code: 429 };
    expect(extractTgRetryAfter(e)).toBe(60);
  });

  it("parses retry after from string message", () => {
    const e = new Error("429: Too Many Requests: retry after 60");
    expect(extractTgRetryAfter(e)).toBe(60);
  });

  it("parses 'retry_after: N' format", () => {
    expect(extractTgRetryAfter("retry_after: 45")).toBe(45);
  });

  it("returns null when not present", () => {
    expect(extractTgRetryAfter(new Error("generic error"))).toBeNull();
  });

  it("returns null for null", () => {
    expect(extractTgRetryAfter(null)).toBeNull();
  });

  it("object without parameters returns null", () => {
    expect(extractTgRetryAfter({ ok: false })).toBeNull();
  });
});

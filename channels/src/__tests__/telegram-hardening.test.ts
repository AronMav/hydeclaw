import { describe, test, expect } from "bun:test";
import { isTgPermanentError } from "../drivers/common";

// ── isTgPermanentError ─────────────────────────────────────────────────

describe("isTgPermanentError", () => {
  // Permanent errors — should NOT be retried

  test("403 Forbidden: bot blocked", () => {
    expect(isTgPermanentError(new Error("403: Forbidden: bot was blocked by the user"))).toBe(true);
  });

  test("403 Forbidden: kicked from group", () => {
    expect(isTgPermanentError(new Error("403: Forbidden: bot was kicked from the supergroup chat"))).toBe(true);
  });

  test("401 Unauthorized", () => {
    expect(isTgPermanentError(new Error("401: Unauthorized"))).toBe(true);
  });

  test("400 chat not found", () => {
    expect(isTgPermanentError(new Error("400: Bad Request: chat not found"))).toBe(true);
  });

  test("400 CHAT_NOT_FOUND", () => {
    expect(isTgPermanentError(new Error("400: Bad Request: CHAT_NOT_FOUND"))).toBe(true);
  });

  test("400 USER_DEACTIVATED", () => {
    expect(isTgPermanentError(new Error("400: Bad Request: USER_DEACTIVATED"))).toBe(true);
  });

  test("400 bot was blocked", () => {
    expect(isTgPermanentError(new Error("400: Bad Request: bot was blocked by the user"))).toBe(true);
  });

  test("400 not enough rights", () => {
    expect(isTgPermanentError(new Error("400: Bad Request: not enough rights to send text messages"))).toBe(true);
  });

  // Transient errors — SHOULD be retried

  test("429 Too Many Requests is transient", () => {
    expect(isTgPermanentError(new Error("429: Too Many Requests: retry after 30"))).toBe(false);
  });

  test("500 Internal Server Error is transient", () => {
    expect(isTgPermanentError(new Error("500: Internal Server Error"))).toBe(false);
  });

  test("ECONNRESET is transient", () => {
    expect(isTgPermanentError(new Error("ECONNRESET"))).toBe(false);
  });

  test("ETIMEDOUT is transient", () => {
    expect(isTgPermanentError(new Error("connect ETIMEDOUT"))).toBe(false);
  });

  test("400 without specific message is transient", () => {
    // Generic 400 without a known permanent pattern should be retried
    expect(isTgPermanentError(new Error("400: Bad Request: message is too long"))).toBe(false);
  });

  // Edge cases

  test("non-Error object", () => {
    expect(isTgPermanentError("403: Forbidden")).toBe(true);
  });

  test("null error is transient", () => {
    expect(isTgPermanentError(null)).toBe(false);
  });

  test("undefined error is transient", () => {
    expect(isTgPermanentError(undefined)).toBe(false);
  });
});

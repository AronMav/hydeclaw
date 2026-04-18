import { describe, it, expect } from "vitest";
import { abortReasonLabel } from "../abort-reason-label";

describe("abortReasonLabel", () => {
  const cases: Array<[string, RegExp]> = [
    ["max_duration", /exceeded.*stream_max_duration/i],
    ["inactivity", /stopped sending data/i],
    ["user_cancelled", /stopped by you/i],
    ["shutdown_drain", /service restart/i],
    ["connect_timeout", /timed out/i],
    ["request_timeout", /timed out/i],
  ];
  for (const [reason, re] of cases) {
    it(`renders ${reason}`, () => {
      expect(abortReasonLabel(reason)).toMatch(re);
    });
  }

  it("handles unknown reason", () => {
    expect(abortReasonLabel("something_new")).toMatch(/aborted.*something_new/i);
  });

  it("handles null/undefined", () => {
    expect(abortReasonLabel(null)).toBe("Aborted.");
    expect(abortReasonLabel(undefined)).toBe("Aborted.");
  });
});

/**
 * Pure-function tests for the collapsed auto-follow FSM.
 *
 * The previous 5-event transition table (user_scroll_up,
 * user_requested_tail, reached_tail, session_switched, stream_started)
 * was indirect — every event was a proxy for "did the viewport leave
 * or return to the tail?". With the IntersectionObserver sentinel
 * signal that question is answered directly, so the FSM reduces to
 * a one-line derivation.
 */

import { describe, it, expect } from "vitest";
import { autoFollowFrom, type AutoFollowState } from "../auto-follow-fsm";

describe("autoFollowFrom", () => {
  it("returns 'on' when the viewport is at the tail", () => {
    expect(autoFollowFrom(true)).toBe<AutoFollowState>("on");
  });

  it("returns 'off' when the viewport is not at the tail", () => {
    expect(autoFollowFrom(false)).toBe<AutoFollowState>("off");
  });

  it("is pure — identical inputs yield identical outputs", () => {
    expect(autoFollowFrom(true)).toBe(autoFollowFrom(true));
    expect(autoFollowFrom(false)).toBe(autoFollowFrom(false));
  });
});

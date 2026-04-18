/**
 * Pure FSM tests. No React, no DOM — just the transition function.
 * Pins every cell of the transition table documented in
 * `docs/superpowers/specs/2026-04-18-auto-follow-fsm-design.md` §4.2.
 */

import { describe, it, expect } from "vitest";
import {
  INITIAL_AUTO_FOLLOW,
  nextAutoFollow,
  type AutoFollowState,
  type AutoFollowEvent,
} from "../auto-follow-fsm";

describe("INITIAL_AUTO_FOLLOW", () => {
  it("starts in `on` so the tail is followed from first render", () => {
    expect(INITIAL_AUTO_FOLLOW).toBe("on");
  });
});

describe("nextAutoFollow — user_scroll_up", () => {
  it("on + user_scroll_up -> off", () => {
    expect(
      nextAutoFollow("on", { type: "user_scroll_up" }),
    ).toBe<AutoFollowState>("off");
  });

  it("off + user_scroll_up -> off (idempotent)", () => {
    expect(
      nextAutoFollow("off", { type: "user_scroll_up" }),
    ).toBe<AutoFollowState>("off");
  });
});

describe("nextAutoFollow — rejoin-tail events (all -> on)", () => {
  const rejoinEvents: AutoFollowEvent[] = [
    { type: "user_requested_tail" },
    { type: "reached_tail" },
    { type: "session_switched" },
    { type: "stream_started" },
  ];

  for (const event of rejoinEvents) {
    it(`on + ${event.type} -> on`, () => {
      expect(nextAutoFollow("on", event)).toBe<AutoFollowState>("on");
    });
    it(`off + ${event.type} -> on`, () => {
      expect(nextAutoFollow("off", event)).toBe<AutoFollowState>("on");
    });
  }
});

describe("nextAutoFollow — determinism", () => {
  it("returns the same next state for the same (state, event) pair", () => {
    const snapshots = Array.from({ length: 10 }, () =>
      nextAutoFollow("off", { type: "reached_tail" }),
    );
    expect(new Set(snapshots).size).toBe(1);
    expect(snapshots[0]).toBe("on");
  });
});

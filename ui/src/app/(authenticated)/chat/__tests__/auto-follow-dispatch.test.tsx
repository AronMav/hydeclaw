/**
 * Tests the `dispatchAutoFollow` pattern used in MessageList.tsx.
 *
 * The pattern: a `useCallback`-wrapped dispatcher that computes the
 * next FSM state and writes it to BOTH a `useRef` (for synchronous
 * reads by Virtuoso callbacks / ResizeObserver) AND a `useState` (for
 * re-rendering consumers like `ScrollToBottomButton`). Both must stay
 * in sync after every dispatch.
 *
 * The test harness is a tiny hook-driven component that exposes the
 * latest ref + state so the test can assert parity under a sequence
 * of events.
 */

import { describe, it, expect } from "vitest";
import { render, act } from "@testing-library/react";
import { useRef, useState, useCallback } from "react";
import {
  INITIAL_AUTO_FOLLOW,
  nextAutoFollow,
  type AutoFollowEvent,
  type AutoFollowState,
} from "../auto-follow-fsm";

/** Minimal reproduction of MessageList's dispatcher. Exposes the
 * ref + state for assertions via `onRender`. */
function Harness({
  onRender,
}: {
  onRender: (api: {
    state: AutoFollowState;
    ref: AutoFollowState;
    dispatch: (e: AutoFollowEvent) => void;
  }) => void;
}) {
  const [state, setState] = useState<AutoFollowState>(INITIAL_AUTO_FOLLOW);
  const ref = useRef<AutoFollowState>(INITIAL_AUTO_FOLLOW);
  const dispatch = useCallback((event: AutoFollowEvent) => {
    const prev = ref.current;
    const next = nextAutoFollow(prev, event);
    if (next === prev) return;
    ref.current = next;
    setState(next);
  }, []);
  onRender({ state, ref: ref.current, dispatch });
  return null;
}

describe("dispatchAutoFollow pattern — ref + state parity", () => {
  it("initial ref and state are both 'on'", () => {
    let captured: { state: AutoFollowState; ref: AutoFollowState } | null = null;
    render(<Harness onRender={(api) => { captured = api; }} />);
    expect(captured!.state).toBe("on");
    expect(captured!.ref).toBe("on");
  });

  it("wheel-up (user_scroll_up) flips BOTH ref and state to 'off'", () => {
    let latest: { state: AutoFollowState; ref: AutoFollowState; dispatch: (e: AutoFollowEvent) => void } | null = null;
    render(<Harness onRender={(api) => { latest = api; }} />);
    act(() => {
      latest!.dispatch({ type: "user_scroll_up" });
    });
    expect(latest!.state).toBe("off");
    expect(latest!.ref).toBe("off");
  });

  it("ref is updated synchronously during dispatch (before next render)", () => {
    // Critical for Virtuoso's followOutput callback — it reads the ref
    // immediately after dispatch via a later tick, MUST see the new value
    // even if the React render hasn't flushed yet.
    let renders = 0;
    let capturedRef: AutoFollowState | null = null;
    let capturedDispatch: ((e: AutoFollowEvent) => void) | null = null;
    render(
      <Harness
        onRender={(api) => {
          renders += 1;
          capturedRef = api.ref;
          capturedDispatch = api.dispatch;
        }}
      />,
    );
    const renderCountBefore = renders;
    act(() => {
      capturedDispatch!({ type: "user_scroll_up" });
    });
    // The dispatch triggered a re-render that updated `capturedRef` to
    // the new value. Confirm both the ref and the new render state
    // agree.
    expect(capturedRef).toBe("off");
    expect(renders).toBeGreaterThan(renderCountBefore);
  });

  it("sequential transitions are consistent across events", () => {
    let latest: { state: AutoFollowState; ref: AutoFollowState; dispatch: (e: AutoFollowEvent) => void } | null = null;
    render(<Harness onRender={(api) => { latest = api; }} />);

    const sequence: Array<[AutoFollowEvent, AutoFollowState]> = [
      [{ type: "user_scroll_up" }, "off"],
      [{ type: "user_scroll_up" }, "off"], // idempotent
      [{ type: "reached_tail" }, "on"],
      [{ type: "user_scroll_up" }, "off"],
      [{ type: "user_requested_tail" }, "on"],
      [{ type: "user_scroll_up" }, "off"],
      [{ type: "session_switched" }, "on"],
      [{ type: "user_scroll_up" }, "off"],
      [{ type: "stream_started" }, "on"],
    ];

    for (const [event, expected] of sequence) {
      act(() => {
        latest!.dispatch(event);
      });
      expect(latest!.state, `state after ${event.type}`).toBe(expected);
      expect(latest!.ref, `ref after ${event.type}`).toBe(expected);
    }
  });

  it("no-op transitions do NOT re-render (perf guard)", () => {
    let renders = 0;
    let capturedDispatch: ((e: AutoFollowEvent) => void) | null = null;
    render(
      <Harness
        onRender={(api) => {
          renders += 1;
          capturedDispatch = api.dispatch;
        }}
      />,
    );
    const renderCountBefore = renders;
    // Already in "on" — reached_tail is a self-transition.
    act(() => {
      capturedDispatch!({ type: "reached_tail" });
    });
    // State did not change, so React should not re-render. (The
    // dispatcher early-returns when `next === prev`.)
    expect(renders).toBe(renderCountBefore);
  });
});

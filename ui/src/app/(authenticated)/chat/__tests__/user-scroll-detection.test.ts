/**
 * Regression tests for the scroll-delta-based user-scroll-up detector.
 *
 * Contract (see `../user-scroll-detection.ts`):
 *   - A `scroll` event whose `scrollTop` decreased by more than
 *     SCROLL_UP_DETECTION_THRESHOLD_PX calls `onUserScrolledUp`.
 *   - Any positive delta (scrolling down, including programmatic
 *     follow-output catch-ups) does NOT fire.
 *   - Sub-threshold negative jitter (browser subpixel rounding,
 *     overflow-anchor adjustments during content growth) does NOT fire.
 *
 * The previous input-typed detector (wheel/touch/keyboard) was too
 * lenient — macOS touchpad inertia tails fired tiny negative `deltaY`
 * wheel events after the user stopped scrolling, falsely dropping
 * auto-follow. The scroll-delta approach is the ground truth
 * regardless of input modality.
 */

import { describe, it, expect, vi } from "vitest";
import {
  attachUserScrollUpDetection,
  SCROLL_UP_DETECTION_THRESHOLD_PX,
} from "../user-scroll-detection";

/** EventTarget-like stand-in with a mutable `scrollTop` so tests can
 * drive the detector without jsdom layout. */
function makeScroller(initialTop = 0) {
  const listeners = new Map<string, Set<EventListener>>();
  return {
    scrollTop: initialTop,
    addEventListener: vi.fn(
      (
        type: string,
        fn: EventListenerOrEventListenerObject,
        _options?: boolean | AddEventListenerOptions,
      ) => {
        if (typeof fn !== "function") return;
        if (!listeners.has(type)) listeners.set(type, new Set());
        listeners.get(type)!.add(fn);
      },
    ),
    removeEventListener: vi.fn(
      (
        type: string,
        fn: EventListenerOrEventListenerObject,
        _options?: boolean | EventListenerOptions,
      ) => {
        if (typeof fn !== "function") return;
        listeners.get(type)?.delete(fn);
      },
    ),
    fire: (type: string) => {
      for (const fn of listeners.get(type) ?? []) fn(new Event(type));
    },
    hasListener: (type: string) => (listeners.get(type)?.size ?? 0) > 0,
  };
}

describe("attachUserScrollUpDetection — scroll-delta contract", () => {
  it("fires when scrollTop decreases by more than the threshold", () => {
    const scroller = makeScroller(1_000);
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.scrollTop = 1_000 - SCROLL_UP_DETECTION_THRESHOLD_PX - 5;
    scroller.fire("scroll");
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("does NOT fire on sub-threshold jitter", () => {
    const scroller = makeScroller(1_000);
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    // Decrease by half the threshold — below the floor.
    scroller.scrollTop = 1_000 - Math.floor(SCROLL_UP_DETECTION_THRESHOLD_PX / 2);
    scroller.fire("scroll");
    expect(cb).not.toHaveBeenCalled();
  });

  it("does NOT fire when scrollTop increases (programmatic follow)", () => {
    const scroller = makeScroller(1_000);
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    // Virtuoso's programmatic follow-output / ResizeObserver scroll-to-bottom
    // drives scrollTop UP toward the tail. Must never trip.
    scroller.scrollTop = 5_000;
    scroller.fire("scroll");
    expect(cb).not.toHaveBeenCalled();
  });

  it("does NOT fire when scrollTop is unchanged", () => {
    const scroller = makeScroller(2_000);
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("scroll"); // no position change
    expect(cb).not.toHaveBeenCalled();
  });

  it("rebaselines between events — repeated small backward steps don't sum up falsely", () => {
    // Three separate tiny jitter-scale decreases must not be aggregated
    // into one trip — each scroll event is evaluated against the
    // immediately-preceding scrollTop, not an old baseline.
    const scroller = makeScroller(2_000);
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    const step = Math.floor(SCROLL_UP_DETECTION_THRESHOLD_PX / 2);
    for (let i = 1; i <= 3; i += 1) {
      scroller.scrollTop = 2_000 - step * i;
      scroller.fire("scroll");
    }
    expect(cb).not.toHaveBeenCalled();
  });

  it("fires again on subsequent real scroll-ups", () => {
    // Idempotency of the detector — firing on every threshold crossing,
    // not just once per attach. The FSM de-dups via its own idempotent
    // `off` state; the detector stays stateless-per-event.
    const scroller = makeScroller(5_000);
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.scrollTop = 4_000;
    scroller.fire("scroll");
    scroller.scrollTop = 3_000;
    scroller.fire("scroll");
    expect(cb).toHaveBeenCalledTimes(2);
  });

  it("ignores non-scroll events entirely", () => {
    // No wheel / keydown / touch listeners are registered — only `scroll`.
    // A wheel event with negative deltaY (which the old detector treated
    // as ground truth and falsely tripped on inertia tails) must have no
    // effect here.
    const scroller = makeScroller(1_000);
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    expect(scroller.hasListener("wheel")).toBe(false);
    expect(scroller.hasListener("keydown")).toBe(false);
    expect(scroller.hasListener("touchmove")).toBe(false);
    scroller.fire("wheel");
    scroller.fire("keydown");
    scroller.fire("touchmove");
    expect(cb).not.toHaveBeenCalled();
  });

  it("teardown removes the scroll listener", () => {
    const scroller = makeScroller(1_000);
    const cb = vi.fn();
    const detach = attachUserScrollUpDetection(scroller, cb);
    detach();
    scroller.scrollTop = 10;
    scroller.fire("scroll");
    expect(cb).not.toHaveBeenCalled();
  });
});

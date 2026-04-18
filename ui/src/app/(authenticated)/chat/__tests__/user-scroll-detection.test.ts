/**
 * Regression tests: during rapid token streaming the previous
 * `isScrolling(false)` + "am I at bottom?" heuristic mistook Virtuoso's
 * own programmatic follow-output scrolls for user scrolls and froze
 * auto-follow partway through long replies. The fix is the pure helper
 * in `../user-scroll-detection.ts` which watches raw input events only —
 * wheel, touch-drag, keyboard. Programmatic `scrollTo` / `scrollToIndex`
 * calls do NOT dispatch wheel/touch/key events, so they cannot falsely
 * trip the flag.
 *
 * These tests pin the contract end-to-end: every input that scrolls
 * content UP calls the callback; every input that scrolls DOWN or sideways
 * does NOT.
 */

import { describe, it, expect, vi } from "vitest";
import { attachUserScrollUpDetection } from "../user-scroll-detection";

/** Minimal EventTarget-like stand-in so we don't need jsdom just for this. */
function makeScroller() {
  const listeners = new Map<string, Set<(e: Event) => void>>();
  return {
    addEventListener: vi.fn((type: string, fn: (e: Event) => void) => {
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type)!.add(fn);
    }),
    removeEventListener: vi.fn((type: string, fn: (e: Event) => void) => {
      listeners.get(type)?.delete(fn);
    }),
    fire: (type: string, event: Event) => {
      for (const fn of listeners.get(type) ?? []) fn(event);
    },
    hasListener: (type: string) => (listeners.get(type)?.size ?? 0) > 0,
  };
}

describe("attachUserScrollUpDetection — wheel", () => {
  it("fires onUserScrolledUp when wheel deltaY is negative (scroll up)", () => {
    const scroller = makeScroller();
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("wheel", { deltaY: -10 } as unknown as WheelEvent);
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("does NOT fire on downward wheel (deltaY positive)", () => {
    const scroller = makeScroller();
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("wheel", { deltaY: 10 } as unknown as WheelEvent);
    expect(cb).not.toHaveBeenCalled();
  });

  it("does NOT fire on zero-deltaY wheel (horizontal-only scroll)", () => {
    const scroller = makeScroller();
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("wheel", { deltaY: 0, deltaX: -50 } as unknown as WheelEvent);
    expect(cb).not.toHaveBeenCalled();
  });
});

describe("attachUserScrollUpDetection — touch", () => {
  function touch(clientY: number): TouchEvent {
    return { touches: [{ clientY } as Touch] } as unknown as TouchEvent;
  }

  it("fires when finger moves DOWN the screen by more than threshold (content scrolls up)", () => {
    const scroller = makeScroller();
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("touchstart", touch(100));
    scroller.fire("touchmove", touch(140)); // finger moved 40px down → scroll up
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("does NOT fire when finger moves UP the screen (content scrolls down)", () => {
    const scroller = makeScroller();
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("touchstart", touch(100));
    scroller.fire("touchmove", touch(40)); // finger moved 60px up → scroll down
    expect(cb).not.toHaveBeenCalled();
  });

  it("does NOT fire on sub-threshold jitter", () => {
    const scroller = makeScroller();
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("touchstart", touch(100));
    scroller.fire("touchmove", touch(103)); // 3px — below the 5px threshold
    expect(cb).not.toHaveBeenCalled();
  });

  it("does NOT fire if touchmove arrives without a prior touchstart", () => {
    const scroller = makeScroller();
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("touchmove", touch(200));
    expect(cb).not.toHaveBeenCalled();
  });
});

describe("attachUserScrollUpDetection — keyboard", () => {
  function key(k: string): KeyboardEvent {
    return { key: k } as unknown as KeyboardEvent;
  }

  it("fires on PageUp / ArrowUp / Home", () => {
    for (const k of ["PageUp", "ArrowUp", "Home"]) {
      const scroller = makeScroller();
      const cb = vi.fn();
      attachUserScrollUpDetection(scroller, cb);
      scroller.fire("keydown", key(k));
      expect(cb, `${k} must trip the flag`).toHaveBeenCalledTimes(1);
    }
  });

  it("does NOT fire on scroll-DOWN keys", () => {
    for (const k of ["PageDown", "ArrowDown", "End"]) {
      const scroller = makeScroller();
      const cb = vi.fn();
      attachUserScrollUpDetection(scroller, cb);
      scroller.fire("keydown", key(k));
      expect(cb, `${k} must NOT trip the flag`).not.toHaveBeenCalled();
    }
  });

  it("does NOT fire on unrelated keys (Enter, letter input)", () => {
    for (const k of ["Enter", "a", "Tab", " "]) {
      const scroller = makeScroller();
      const cb = vi.fn();
      attachUserScrollUpDetection(scroller, cb);
      scroller.fire("keydown", key(k));
      expect(cb).not.toHaveBeenCalled();
    }
  });
});

describe("attachUserScrollUpDetection — programmatic scrolls are immune", () => {
  it("does not fire when no wheel/touch/key events are dispatched", () => {
    // This is the regression guard: scrollTo / scrollToIndex / programmatic
    // scroll do NOT dispatch wheel, touch, or keydown events — only `scroll`
    // events (which this helper intentionally does NOT listen for). A
    // `scroll` event firing must be a no-op for user-scroll-up detection.
    const scroller = makeScroller();
    const cb = vi.fn();
    attachUserScrollUpDetection(scroller, cb);
    scroller.fire("scroll", new Event("scroll"));
    expect(cb).not.toHaveBeenCalled();
    // And the helper never registered a `scroll` listener — double-check.
    expect(scroller.hasListener("scroll")).toBe(false);
  });
});

describe("attachUserScrollUpDetection — teardown", () => {
  it("removes all registered listeners", () => {
    const scroller = makeScroller();
    const cb = vi.fn();
    const detach = attachUserScrollUpDetection(scroller, cb);
    detach();
    scroller.fire("wheel", { deltaY: -50 } as unknown as WheelEvent);
    scroller.fire("keydown", { key: "PageUp" } as unknown as KeyboardEvent);
    scroller.fire("touchstart", {
      touches: [{ clientY: 100 } as Touch],
    } as unknown as TouchEvent);
    scroller.fire("touchmove", {
      touches: [{ clientY: 300 } as Touch],
    } as unknown as TouchEvent);
    expect(cb).not.toHaveBeenCalled();
  });
});

/**
 * Regression tests: the scroll-to-bottom button used to land at the TOP
 * of the last message (because `scrollToIndex({index: "LAST"})` without
 * `align: "end"` anchors the START of the item to the viewport). For a
 * 5000-word assistant message this left the user thousands of pixels
 * above the actual chat bottom.
 *
 * These tests pin the fix:
 *   - `runScrollToBottom` MUST call `scrollToIndex({index: "LAST", align: "end"})`
 *     to anchor the last item's bottom edge to the viewport.
 *   - It MUST then follow up with an absolute `scrollTo({top: MAX_SAFE_INTEGER})`
 *     to cover any trailing content (virtuoso Footer, ThinkingMessage,
 *     streaming indicator) that sits below the last data row.
 *   - It MUST be a no-op when the handle is null (caller can fall back).
 */

import { describe, it, expect, vi } from "vitest";
import { runScrollToBottom, type VirtuosoLike } from "../scroll-to-bottom";

type Handle = VirtuosoLike & {
  scrollToIndex: ReturnType<typeof vi.fn>;
  scrollTo: ReturnType<typeof vi.fn>;
};

function makeHandle(): Handle {
  const scrollToIndex = vi.fn() as Handle["scrollToIndex"];
  const scrollTo = vi.fn() as Handle["scrollTo"];
  return { scrollToIndex, scrollTo };
}

describe("runScrollToBottom", () => {
  it("anchors last item's END to viewport (not its top)", () => {
    const handle = makeHandle();
    runScrollToBottom(handle, { schedule: () => {} });
    expect(handle.scrollToIndex).toHaveBeenCalledTimes(1);
    const arg = handle.scrollToIndex.mock.calls[0][0];
    expect(arg).toMatchObject({
      index: "LAST",
      align: "end",
    });
    // Without align:"end" the regression re-surfaces — guard explicitly.
    expect(arg.align).toBe("end");
  });

  it("follows up with absolute scrollTo MAX_SAFE_INTEGER", () => {
    const handle = makeHandle();
    // Run the follow-up callback synchronously so we can assert.
    runScrollToBottom(handle, { schedule: (fn) => fn() });
    expect(handle.scrollTo).toHaveBeenCalledTimes(1);
    const arg = handle.scrollTo.mock.calls[0][0];
    expect(arg.top).toBe(Number.MAX_SAFE_INTEGER);
  });

  it("schedules the follow-up scrollTo with a delay, not inline", () => {
    const handle = makeHandle();
    const schedule = vi.fn();
    runScrollToBottom(handle, { schedule, followupDelayMs: 120 });
    expect(schedule).toHaveBeenCalledTimes(1);
    // Second argument is the delay; we want a non-zero value so Virtuoso
    // has time to finish the scrollToIndex layout before we jump.
    expect(schedule.mock.calls[0][1]).toBeGreaterThan(0);
    // The absolute scrollTo must NOT have fired yet (it's scheduled).
    expect(handle.scrollTo).not.toHaveBeenCalled();
  });

  it("returns false and does nothing when handle is null", () => {
    const result = runScrollToBottom(null);
    expect(result).toBe(false);
  });

  it("returns false and does nothing when handle is undefined", () => {
    const result = runScrollToBottom(undefined);
    expect(result).toBe(false);
  });

  it("returns true when handle is provided", () => {
    const handle = makeHandle();
    expect(runScrollToBottom(handle, { schedule: () => {} })).toBe(true);
  });

  it("uses smooth scroll for both calls (UX: don't teleport)", () => {
    const handle = makeHandle();
    runScrollToBottom(handle, { schedule: (fn) => fn() });
    expect(handle.scrollToIndex.mock.calls[0][0].behavior).toBe("smooth");
    expect(handle.scrollTo.mock.calls[0][0].behavior).toBe("smooth");
  });
});

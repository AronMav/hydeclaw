/**
 * User-initiated scroll-up detection for the chat Virtuoso scroller.
 *
 * Context: auto-follow during streaming is gated on
 * `!userScrolledUpRef.current`. The flag must flip to `true` ONLY when
 * the user actually scrolls up, never when Virtuoso programmatically
 * scrolls to catch up with rapid token arrivals.
 *
 * The previous implementation used Virtuoso's `isScrolling(false)` callback
 * with a "am I at bottom right now?" check. That signal fires for BOTH
 * user scrolls and programmatic follow-output scrolls — under rapid
 * streaming it transiently reports `isAtBottom=false` during programmatic
 * catch-up, falsely marking the flag and freezing auto-follow.
 *
 * The reliable signal is raw input events (wheel / touch / keyboard) on
 * the scroller. We listen passively so we never block the actual scroll.
 *
 * Exported as a pure function so the contract is unit-testable without a
 * full component render (see `__tests__/user-scroll-detection.test.ts`).
 */

type EventTargetLike = {
  addEventListener: (
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: AddEventListenerOptions | boolean,
  ) => void;
  removeEventListener: (
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: boolean | EventListenerOptions,
  ) => void;
};

/** Keys that scroll the content up and therefore signal user intent to
 * leave the live tail. `PageDown` / `ArrowDown` / `End` scroll DOWN and
 * must not trip the flag. */
const SCROLL_UP_KEYS = new Set(["PageUp", "ArrowUp", "Home"]);

/** Minimum finger travel (px) to register a touch-scroll as user intent.
 * Below this threshold we ignore jitter from a stationary touch. */
const TOUCH_SCROLL_THRESHOLD_PX = 5;

/** Attach input listeners that call `onUserScrolledUp` when the user
 * explicitly scrolls up via wheel, touch-drag, or keyboard. Returns a
 * teardown function that removes all listeners. */
export function attachUserScrollUpDetection(
  scroller: EventTargetLike,
  onUserScrolledUp: () => void,
): () => void {
  const onWheel = (e: Event) => {
    const we = e as WheelEvent;
    if (we.deltaY < 0) onUserScrolledUp();
  };

  let touchStartY: number | null = null;
  const onTouchStart = (e: Event) => {
    const te = e as TouchEvent;
    touchStartY = te.touches[0]?.clientY ?? null;
  };
  const onTouchMove = (e: Event) => {
    const te = e as TouchEvent;
    const y = te.touches[0]?.clientY ?? null;
    if (touchStartY !== null && y !== null && y > touchStartY + TOUCH_SCROLL_THRESHOLD_PX) {
      // Finger moved DOWN the screen, which scrolls content UP.
      onUserScrolledUp();
    }
  };

  const onKeyDown = (e: Event) => {
    const ke = e as KeyboardEvent;
    if (SCROLL_UP_KEYS.has(ke.key)) onUserScrolledUp();
  };

  scroller.addEventListener("wheel", onWheel, { passive: true });
  scroller.addEventListener("touchstart", onTouchStart, { passive: true });
  scroller.addEventListener("touchmove", onTouchMove, { passive: true });
  scroller.addEventListener("keydown", onKeyDown);

  return () => {
    scroller.removeEventListener("wheel", onWheel);
    scroller.removeEventListener("touchstart", onTouchStart);
    scroller.removeEventListener("touchmove", onTouchMove);
    scroller.removeEventListener("keydown", onKeyDown);
  };
}

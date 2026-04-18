/**
 * User-initiated scroll-up detection for the chat Virtuoso scroller.
 *
 * Context: auto-follow is gated on the FSM in `./auto-follow-fsm.ts`.
 * The FSM must transition `on → off` ONLY when the user actually pulls
 * the viewport away from the live tail, never when Virtuoso or the
 * ResizeObserver path does a programmatic catch-up scroll.
 *
 * History:
 * 1. First attempt: Virtuoso's `isScrolling(false)` + "am I at bottom?"
 *    — false positives during rapid streaming because the at-bottom
 *    signal flipped while programmatic follow was still catching up.
 * 2. Second attempt: wheel / touch / keyboard input-event detection —
 *    false positives from macOS touchpad inertia-tail (tiny negative
 *    deltaY events after the user stops), from `keydown` bubbling
 *    when focus landed on a button inside a message, and from
 *    touch-move smoothing on iOS.
 * 3. Current: raw `scroll` event + net `scrollTop` delta. A negative
 *    delta beyond a small threshold means the viewport actually moved
 *    UP — that is the ground truth for user scroll-up regardless of
 *    modality (wheel, touchpad, keyboard, scrollbar drag, pinch, etc).
 *    Programmatic scrolls (Virtuoso follow-output, ResizeObserver
 *    scroll-to-bottom, manual scroll-to-bottom button) only ever
 *    INCREASE scrollTop, so they cannot fire a false positive.
 *
 * Exported as a pure function so the contract is unit-testable without
 * a full component render (see
 * `__tests__/user-scroll-detection.test.ts`).
 */

/** Minimum scrollTop decrease (in pixels) that counts as a deliberate
 * user scroll-up. Anything smaller is treated as jitter — browser
 * subpixel rounding, overflow-anchor adjustments, layout reflow during
 * rapid streaming. 10 px is smaller than a single wheel tick on every
 * platform so a real wheel-up always clears this; large enough to
 * swallow the 1–2 px noise from anchor-based content growth. */
const SCROLL_UP_THRESHOLD_PX = 10;

type ScrollerLike = {
  scrollTop: number;
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

/** Attach a `scroll` listener that calls `onUserScrolledUp` when the
 * viewport's `scrollTop` decreases by more than the jitter threshold.
 * Returns a teardown function that removes the listener. */
export function attachUserScrollUpDetection(
  scroller: ScrollerLike,
  onUserScrolledUp: () => void,
): () => void {
  let prevTop = scroller.scrollTop;
  const onScroll = () => {
    const newTop = scroller.scrollTop;
    if (newTop < prevTop - SCROLL_UP_THRESHOLD_PX) {
      onUserScrolledUp();
    }
    prevTop = newTop;
  };
  scroller.addEventListener("scroll", onScroll, { passive: true });
  return () => {
    scroller.removeEventListener("scroll", onScroll);
  };
}

/** Exported for tests that want to assert the exact threshold. */
export const SCROLL_UP_DETECTION_THRESHOLD_PX = SCROLL_UP_THRESHOLD_PX;

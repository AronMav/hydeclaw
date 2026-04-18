/**
 * Geometric tail-detection for the chat Virtuoso scroller.
 *
 * Design: `docs/superpowers/specs/2026-04-18-tail-sentinel-auto-follow-design.md`
 *
 * Previous approaches (isScrolling + isAtBottom, wheel/touch/keydown
 * detector, scroll-delta detector + direct scrollTop pin) all shared a
 * failure mode: they inferred "am I at the tail?" from `scrollTop` /
 * `scrollHeight` measurements, and those measurements are unreliable
 * under Virtuoso's virtualization (item shuffling, internal-container
 * replacement, catch-up lag).
 *
 * This module replaces that inference with a geometric observation:
 * place a 1 px sentinel inside Virtuoso's Footer (which never
 * virtualizes), point an IntersectionObserver at it with a 200 px
 * bottom rootMargin, and forward the resulting `isIntersecting`
 * booleans. The signal is independent of scrollTop values, modality
 * (wheel / trackpad / keyboard / scrollbar / programmatic), and
 * Virtuoso's internal render cycle.
 */

export interface TailSentinelOptions {
  /** Bottom rootMargin for the IntersectionObserver. Defaults to
   * "200px 0px" — the sentinel counts as "in the viewport" while it
   * sits anywhere in the bottom 200 px of the scroller. Matches
   * Virtuoso's default `atBottomThreshold` for obvious semantics. */
  rootMargin?: string;
}

export const DEFAULT_TAIL_ROOT_MARGIN = "200px 0px";

/**
 * Attach an IntersectionObserver to `sentinel` with `scroller` as root.
 * Invokes `onTailStateChange(isAtTail)` on every intersection transition
 * reported by the browser. Returns a teardown that disconnects the
 * observer.
 */
export function attachTailSentinel(
  scroller: Element,
  sentinel: Element,
  onTailStateChange: (isAtTail: boolean) => void,
  options: TailSentinelOptions = {},
): () => void {
  const observer = new IntersectionObserver(
    (entries) => {
      // We observe exactly one sentinel per attach, so we only care
      // about the first (and only) entry. Reading [0] explicitly
      // makes that invariant self-documenting.
      const entry = entries[0];
      if (entry) onTailStateChange(entry.isIntersecting);
    },
    {
      root: scroller,
      rootMargin: options.rootMargin ?? DEFAULT_TAIL_ROOT_MARGIN,
      threshold: 0,
    },
  );
  observer.observe(sentinel);
  return () => observer.disconnect();
}

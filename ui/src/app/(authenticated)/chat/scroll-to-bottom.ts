/**
 * Scroll-to-bottom helper extracted for testability.
 *
 * The naive `virtuosoRef.current?.scrollToIndex({index: "LAST"})` scrolls to
 * the TOP of the last item. For a long assistant message that lands at the
 * message's beginning rather than its end — the user clicks "scroll to
 * bottom" and sees the start of the last message instead of the actual
 * bottom of the chat.
 *
 * Correct behavior: anchor the last item's END to the viewport
 * (`align: "end"`), then issue a follow-up absolute scroll to cover any
 * trailing content (ThinkingMessage, virtuoso Footer, streaming indicator,
 * etc.) that sits below the last data row.
 */

/** Subset of the VirtuosoHandle surface we actually touch. */
export interface VirtuosoLike {
  scrollToIndex: (opts: {
    index: number | "LAST";
    align?: "start" | "center" | "end";
    behavior?: "auto" | "smooth";
  }) => void;
  scrollTo: (opts: { top: number; behavior?: "auto" | "smooth" }) => void;
}

export interface ScrollToBottomOptions {
  /** Delay before the follow-up absolute `scrollTo` fires. Matches the
   * ~100 ms Virtuoso typically needs to finish a `scrollToIndex` layout.
   * Configurable for tests. */
  followupDelayMs?: number;
  /** Optional scheduler override (tests may pass a synchronous one). */
  schedule?: (fn: () => void, ms: number) => unknown;
}

/** Execute the scroll-to-bottom sequence against a VirtuosoHandle-like ref.
 * Returns `false` if `handle` is null (caller can fall back to e.g.
 * scrolling the container itself). */
export function runScrollToBottom(
  handle: VirtuosoLike | null | undefined,
  opts: ScrollToBottomOptions = {},
): boolean {
  if (!handle) return false;
  handle.scrollToIndex({ index: "LAST", align: "end", behavior: "smooth" });
  const schedule = opts.schedule ?? setTimeout;
  schedule(() => {
    handle.scrollTo({ top: Number.MAX_SAFE_INTEGER, behavior: "smooth" });
  }, opts.followupDelayMs ?? 120);
  return true;
}

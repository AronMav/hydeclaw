/**
 * Auto-follow finite state machine for the chat message list.
 *
 * Problem: the old two-signal approach (`userScrolledUpRef` + `isAtBottom`)
 * conflated "user intent to leave the live tail" with "viewport is
 * currently near the tail". Under rapid streaming, Virtuoso programmatic
 * follow-output scrolls lagged behind token arrivals; `isAtBottom`
 * transiently flipped false; the scroll-to-bottom button materialized
 * and the ResizeObserver-driven auto-scroll stopped firing, even though
 * the user never interacted with the viewport. See
 * `docs/superpowers/specs/2026-04-18-auto-follow-fsm-design.md`.
 *
 * Fix: this module encodes auto-follow as a single FSM with state
 * `"on" | "off"`. All three consumers (Virtuoso followOutput,
 * ResizeObserver scroll path, ScrollToBottomButton visibility) read
 * this state and nothing else. `isAtBottom` is demoted to an
 * informational signal (badge reset only).
 *
 * The FSM is a pure function with zero React / zero DOM dependencies,
 * so it is unit-testable without render infrastructure.
 */

export type AutoFollowState = "on" | "off";

export type AutoFollowEvent =
  /** User scrolled up via wheel, touch-drag, or PageUp/ArrowUp/Home.
   * The only way to leave the live tail. */
  | { type: "user_scroll_up" }
  /** User clicked the scroll-to-bottom button — explicit request to
   * rejoin the tail. */
  | { type: "user_requested_tail" }
  /** Virtuoso's `atBottomStateChange(true)` fired — the viewport is
   * physically at the tail (user scrolled naturally or layout settled
   * after a programmatic scroll). */
  | { type: "reached_tail" }
  /** Session id changed — treat the new session as a fresh view and
   * reset to auto-follow. */
  | { type: "session_switched" }
  /** User sent a new prompt and the stream is about to start — caller
   * wants to watch the reply live regardless of prior state. */
  | { type: "stream_started" };

/** Initial FSM state used by the component on mount. */
export const INITIAL_AUTO_FOLLOW: AutoFollowState = "on";

/**
 * Pure transition function. `on → off` requires explicit user intent
 * (`user_scroll_up`). Any other event returns to `on`, making the
 * "rejoin tail" paths idempotent.
 *
 * The `switch` is exhaustive over `AutoFollowEvent`; adding a new
 * variant without updating this function is a TypeScript error.
 */
export function nextAutoFollow(
  _state: AutoFollowState,
  event: AutoFollowEvent,
): AutoFollowState {
  switch (event.type) {
    case "user_scroll_up":
      return "off";
    case "user_requested_tail":
    case "reached_tail":
    case "session_switched":
    case "stream_started":
      return "on";
  }
}

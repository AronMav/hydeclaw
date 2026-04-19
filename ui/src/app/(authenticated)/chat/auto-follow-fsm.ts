/**
 * Derived auto-follow state for the chat message list.
 *
 * History: `docs/superpowers/specs/2026-04-18-auto-follow-fsm-design.md`
 * proposed a 5-event FSM to replace a two-signal-conflation bug. Under
 * rapid streaming the FSM itself proved insufficient — every detector
 * that fed it `user_scroll_up` eventually misfired. The current design
 * (`docs/superpowers/specs/2026-04-18-tail-sentinel-auto-follow-design.md`)
 * replaces inference with direct geometric observation of the tail,
 * reducing this module to a single boolean projection.
 *
 * Kept as a named function + type for readability at call sites and
 * to preserve a single place to evolve the derivation in the future.
 */

export type AutoFollowState = "on" | "off";

/** Derive the auto-follow state from the tail-sentinel boolean. */
export function autoFollowFrom(isAtTail: boolean): AutoFollowState {
  return isAtTail ? "on" : "off";
}

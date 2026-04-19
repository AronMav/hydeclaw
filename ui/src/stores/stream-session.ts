"use client";

import { useChatStore } from "./chat-store";
import type { AgentState } from "./chat-types";

/**
 * Invariant-enforcing wrapper around stream-state mutations.
 *
 * A `StreamSession` owns three things for the lifetime of one
 * streaming operation:
 *   1. A generation number — set at construction, compared on every
 *      write. Writes from a session whose generation has been
 *      superseded (another `streamSessionManager.start()` ran for the
 *      same agent) are silently dropped.
 *   2. An `AbortSignal` — passed to fetch. Disposing the session
 *      aborts the signal.
 *   3. A reference to the shared Zustand store — writes go through
 *      `useChatStore.setState` internally; no raw store reference is exposed.
 *
 * Stream-touching state (messageSource live-mode, connectionPhase,
 * connectionError, streamError, streamGeneration, reconnectAttempt)
 * MUST be written via this class. Non-stream state (activeSessionId,
 * selectedBranches, renderLimit, turnLimitMessage, etc.) can be
 * written directly from navigation/CRUD actions.
 *
 * See `docs/superpowers/specs/2026-04-19-chat-architecture-cleanup-design.md` §7.
 */
export class StreamSession {
  readonly agent: string;
  readonly generation: number;

  #controller: AbortController;
  #disposed = false;

  constructor(agent: string, generation: number) {
    this.agent = agent;
    this.generation = generation;
    this.#controller = new AbortController();
  }

  get signal(): AbortSignal {
    return this.#controller.signal;
  }

  get disposed(): boolean {
    return this.#disposed;
  }

  get isCurrent(): boolean {
    if (this.#disposed) return false;
    const current = useChatStore.getState().agents[this.agent]?.streamGeneration ?? 0;
    return current === this.generation;
  }

  write(patch: Partial<AgentState>): void {
    if (!this.isCurrent) {
      if (process.env.NODE_ENV !== "production") {
        // eslint-disable-next-line no-console
        console.debug(
          `[StreamSession] dropped write for agent=${this.agent} gen=${this.generation}`,
          patch,
        );
      }
      return;
    }
    useChatStore.setState((draft: any) => {
      const st = draft.agents[this.agent];
      if (!st) return;
      Object.assign(st, patch);
    });
  }

  writeDraft(mutator: (agentDraft: AgentState) => void): void {
    if (!this.isCurrent) {
      if (process.env.NODE_ENV !== "production") {
        // eslint-disable-next-line no-console
        console.debug(
          `[StreamSession] dropped writeDraft for agent=${this.agent} gen=${this.generation}`,
        );
      }
      return;
    }
    useChatStore.setState((draft: any) => {
      const st = draft.agents[this.agent];
      if (!st) return;
      mutator(st);
    });
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#controller.abort();
    // Final legal stream-state write BEFORE generation bump: land
    // `connectionPhase: "idle"` so consumers that read the phase see
    // the terminal state. This is the ONLY stream-state write that
    // legitimately happens during dispose; all other writes during
    // teardown go through the invariant-protected `write()` / bail
    // on `isCurrent`. Bypassing `isCurrent` here is safe because
    // `dispose()` is the only caller of this internal path and it
    // runs exactly once (idempotency guard above).
    useChatStore.setState((draft: any) => {
      const st = draft.agents[this.agent];
      if (!st) return;
      st.connectionPhase = "idle";
      st.streamGeneration = (st.streamGeneration ?? 0) + 1;
    });
  }
}

/**
 * Per-agent StreamSession registry. `start()` creates a new session,
 * auto-disposing any previous one for the same agent. `current()`
 * returns the active session or null. `disposeCurrent()` is the
 * navigation-initiated teardown path (equivalent to `abortLocalOnly`
 * in the legacy renderer).
 */
const activeSessions = new Map<string, StreamSession>();

export const streamSessionManager = {
  start(agent: string): StreamSession {
    // Single generation bump per logical transition: dispose the
    // previous session (which bumps), then create the new session
    // whose generation reflects the bumped value.
    const previous = activeSessions.get(agent);
    if (previous) {
      previous.dispose();
    } else {
      // No previous session to dispose — bump the generation directly
      // so that `start()` always advances the counter exactly once.
      useChatStore.setState((draft: any) => {
        const st = draft.agents[agent];
        if (!st) return;
        st.streamGeneration = (st.streamGeneration ?? 0) + 1;
      });
    }

    const nextGen = useChatStore.getState().agents[agent]?.streamGeneration ?? 0;
    const session = new StreamSession(agent, nextGen);
    activeSessions.set(agent, session);
    return session;
  },

  current(agent: string): StreamSession | null {
    const s = activeSessions.get(agent);
    return s && !s.disposed ? s : null;
  },

  disposeCurrent(agent: string): void {
    const s = activeSessions.get(agent);
    if (!s) return;
    s.dispose();
    activeSessions.delete(agent);
  },
};

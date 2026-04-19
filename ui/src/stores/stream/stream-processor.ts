// ── stream/stream-processor.ts ──────────────────────────────────────────────
// SSE event-dispatch loop extracted from streaming-renderer.ts (Task 4.3).
// Reads a ReadableStream<Uint8Array>, parses SSE frames, and dispatches each
// event to the appropriate StreamSession write path.
// Callers inject reconnect + sessionId callbacks so this module has
// no dependency on the renderer's closure state.

import { parseSSELines, parseSseEvent } from "./sse-parser";
import { parseContentParts } from "@/stores/sse-events";
import { IncrementalParser } from "@/lib/message-parser";
import { queryClient } from "@/lib/query-client";
import { qk } from "@/lib/queries";
import type { SessionRow } from "@/types/api";

import { uuid, STREAM_THROTTLE_MS, getLiveMessages } from "../chat-types";
import type {
  ChatMessage,
  MessagePart,
  TextPart,
  ToolPart,
  ApprovalPart,
  ConnectionPhase,
  AgentState,
} from "../chat-types";
import type { StreamSession } from "../stream-session";

// ── Public interface ──────────────────────────────────────────────────────────

export interface StreamProcessorCallbacks {
  /** Called when a `data-session-id` event arrives. */
  onSessionId: (sid: string) => void;
  /**
   * Called when the stream ends without a finish event — the caller decides
   * the reconnect policy (scheduleReconnect in streaming-renderer.ts).
   */
  onReconnectNeeded: (sid: string, attempt: number) => void;
  /**
   * Read current agent state from the store. Injected to avoid circular import
   * (chat-store → streaming-renderer → stream-processor → chat-store).
   */
  getAgentState: (agent: string) => AgentState | undefined;
  /**
   * Call updateSessionParticipants on the store. Injected for the same reason.
   */
  updateSessionParticipants: (sessionId: string, participants: string[]) => void;
  /**
   * Called when the stream ends cleanly (not aborted, not reconnect). Used by
   * the renderer to persist UI state (debounced PATCH to backend).
   */
  onStreamDone?: () => void;
}

export interface StreamProcessorOpts {
  sessionId: string | null;
  reconnectAttempt: number;
  callbacks: StreamProcessorCallbacks;
}

// ── Core processor ─────────────────────────────────────────────────────────────

export async function processSSEStream(
  session: StreamSession,
  body: ReadableStream<Uint8Array>,
  opts: StreamProcessorOpts,
): Promise<void> {
  const { sessionId: knownSessionId, reconnectAttempt, callbacks } = opts;
  const agent = session.agent;

  const reader = body.getReader();
  const decoder = new TextDecoder();
  const buffer = { current: "" };

  // Mutable assistant message being built
  let assistantId = uuid();
  let assistantCreatedAt = new Date().toISOString();
  let parts: MessagePart[] = [];
  const incrementalParser = new IncrementalParser();
  const toolInputChunks = new Map<string, string[]>();
  let receivedSessionId: string | null = knownSessionId ?? null;
  // Track finish event to distinguish natural end from connection drop
  let receivedFinishEvent = false;
  let currentRespondingAgent: string | null = agent;

  function flushText() {
    const flushed = incrementalParser.flush();
    if (flushed.length > 0) {
      parts.push(...flushed);
    }
  }

  function pushUpdate() {
    // Guard: stale stream -- a newer stream has started, discard updates
    if (!session.isCurrent) return;
    // Guard: don't update store after abort (prevents race with stopStream)
    if (session.signal.aborted) return;

    const textParts = incrementalParser.snapshot();
    const nonTextParts = parts.filter(p => p.type !== "text" && p.type !== "reasoning");

    session.writeDraft((agentDraft: AgentState) => {
      // Double-check generation inside draft to close race window
      if ((agentDraft as any).streamGeneration !== session.generation) return;
      if (agentDraft.messageSource.mode !== "live") {
        agentDraft.messageSource = { mode: "live", messages: [] };
      }
      // Preserve existing overlay messages (e.g. optimistic user msg)
      const liveMessages = (agentDraft.messageSource as any).messages as ChatMessage[];
      const existing = liveMessages.findIndex((m: ChatMessage) => m.id === assistantId);

      const allParts = [...nonTextParts, ...textParts] as MessagePart[];

      if (existing >= 0) {
        const msg = liveMessages[existing];
        msg.parts = allParts;
        msg.agentId = currentRespondingAgent ?? undefined;
      } else {
        liveMessages.push({
          id: assistantId,
          role: "assistant",
          parts: allParts,
          createdAt: assistantCreatedAt,
          agentId: currentRespondingAgent ?? undefined,
        });
      }
      if (agentDraft.connectionPhase !== "error") agentDraft.connectionPhase = "streaming";
    });
  }

  // Throttle UI updates to ~20fps (50ms) -- reduces renders from 60/sec to 20/sec.
  // setTimeout coalesces rapid SSE events, then rAF syncs with browser paint cycle.
  let updateScheduled = false;
  let updateTimer: ReturnType<typeof setTimeout> | null = null;
  function scheduleUpdate() {
    if (updateScheduled) return;
    updateScheduled = true;
    updateTimer = setTimeout(() => {
      updateTimer = null;
      requestAnimationFrame(() => {
        updateScheduled = false;
        pushUpdate();
      });
    }, STREAM_THROTTLE_MS);
  }
  function cancelScheduledUpdate() {
    if (updateTimer) { clearTimeout(updateTimer); updateTimer = null; }
    updateScheduled = false;
  }

  try {
    while (true) {
      if (session.signal.aborted) break;
      const { done, value } = await reader.read();
      if (done) break;

      const chunk = decoder.decode(value, { stream: true });
      const lines = parseSSELines(chunk, buffer);

      for (const line of lines) {
        if (!line.startsWith("data:")) continue;
        const raw = line.slice(5).trim();
        if (raw === "[DONE]") continue;

        const event = parseSseEvent(raw);
        if (!event) {
          if (process.env.NODE_ENV !== "production") console.warn("[sse] unparseable event:", raw.slice(0, 120));
          continue;
        }

        // Stale-stream short-circuit: if abort fired or streamGeneration
        // moved (navigation / new stream), drop any remaining events
        // buffered inside the current chunk. Individual case-level
        // guards (data-session-id, pushUpdate) are still in place; this
        // is the belt-and-suspenders catch for `sync`-terminal and
        // `error` paths that do unconditional `session.write(...)`.
        if (session.signal.aborted || !session.isCurrent) {
          continue;
        }

        switch (event.type) {
          case "data-session-id": {
            const sid = event.data.sessionId;
            if (sid && session.isCurrent) {
              receivedSessionId = sid;
              // SSE-03: Confirm the optimistic user message
              session.writeDraft((agentDraft: AgentState) => {
                if (agentDraft.messageSource.mode !== "live") return;
                const msgs = (agentDraft.messageSource as any).messages as ChatMessage[];
                for (let i = msgs.length - 1; i >= 0; i--) {
                  if (msgs[i].role === "user" && msgs[i].status === "sending") {
                    msgs[i].status = "confirmed";
                    break;
                  }
                }
              });
              session.write({ activeSessionId: sid });
              // saveLastSession is called from chat-store.ts via the callback
              callbacks.onSessionId(sid);

              // Populate sessionParticipants cache from React Query session data
              const sessionsData = queryClient.getQueryData<{ sessions: SessionRow[] }>(
                qk.sessions(agent)
              );
              const cachedSession = sessionsData?.sessions.find(s => s.id === sid);
              if (cachedSession?.participants) {
                callbacks.updateSessionParticipants(sid, cachedSession.participants);
              }
            }
            break;
          }

          case "start": {
            const newId = event.messageId || assistantId;

            // Don't reset if current message only has tool/approval parts —
            // Architecture C: each `start` = new LLM iteration.
            // Reset overlay for new assistant message. History refresh happens
            // via React Query polling (3-5s) — no forced invalidation to avoid races.
            assistantId = newId;
            assistantCreatedAt = new Date().toISOString();
            parts = [];
            incrementalParser.reset();
            if (event.agentName) currentRespondingAgent = event.agentName;
            break;
          }

          case "text-start": {
            if (event.agentName) currentRespondingAgent = event.agentName;
            break;
          }

          case "text-delta": {
            incrementalParser.processDelta(event.delta);
            scheduleUpdate();
            break;
          }

          case "text-end": {
            scheduleUpdate();
            break;
          }

          case "tool-input-start": {
            flushText();
            const { toolCallId: tcId, toolName: tcName } = event;
            toolInputChunks.set(tcId, []);
            const toolPart: ToolPart = {
              type: "tool",
              toolCallId: tcId,
              toolName: tcName,
              state: "input-streaming",
              input: {},
            };
            parts.push(toolPart);
            scheduleUpdate();
            break;
          }

          case "tool-input-delta": {
            const { toolCallId: tcId, inputTextDelta: delta } = event;
            if (delta) toolInputChunks.get(tcId)?.push(delta);
            break;
          }

          case "tool-input-available": {
            const { toolCallId: tcId, input } = event;
            toolInputChunks.delete(tcId);
            const idx = parts.findIndex(
              (p) => p.type === "tool" && p.toolCallId === tcId,
            );
            if (idx >= 0) {
              parts[idx] = { ...(parts[idx] as ToolPart), state: "input-available", input: (input as Record<string, unknown>) ?? {} };
            }
            scheduleUpdate();
            break;
          }

          case "tool-output-available": {
            const { toolCallId: tcId, output } = event;
            const idx = parts.findIndex(
              (p) => p.type === "tool" && p.toolCallId === tcId,
            );
            if (idx >= 0) {
              parts[idx] = { ...(parts[idx] as ToolPart), state: "output-available", output };
            }
            scheduleUpdate();
            break;
          }

          case "tool-approval-needed": {
            flushText();
            const approval: ApprovalPart = {
              type: "approval",
              approvalId: event.approvalId,
              toolName: event.toolName,
              toolInput: event.toolInput,
              timeoutMs: event.timeoutMs,
              receivedAt: Date.now(),
              status: "pending",
            };
            parts.push(approval);
            scheduleUpdate();
            break;
          }

          case "tool-approval-resolved": {
            const idx = parts.findIndex(
              (p) => p.type === "approval" && p.approvalId === event.approvalId,
            );
            if (idx >= 0) {
              const existing = parts[idx] as ApprovalPart;
              parts[idx] = {
                ...existing,
                status: event.action,
                modifiedInput: event.modifiedInput,
              };
            }
            scheduleUpdate();
            break;
          }

          case "step-start":
          case "step-finish":
            // Step groups removed — tools render as flat parts (matching history view)
            break;

          case "file": {
            flushText();
            parts.push({
              type: "file",
              url: event.url,
              mediaType: event.mediaType || "application/octet-stream",
            });
            scheduleUpdate();
            break;
          }

          case "rich-card": {
            flushText();
            parts.push({
              type: "rich-card",
              cardType: event.cardType,
              data: event.data,
            });
            scheduleUpdate();
            break;
          }

          case "sync": {
            const { content: syncContent, status: syncStatus } = event;
            // Architecture C: overlay = text only. Tools come from history (DB).
            const syncParts: MessagePart[] = parseContentParts(syncContent || "");

            session.writeDraft((agentDraft: AgentState) => {
              // Guard: skip sync events for a different session (e.g. agent switch race)
              if (receivedSessionId && agentDraft.activeSessionId && receivedSessionId !== agentDraft.activeSessionId) return;

              const currentSessionId = agentDraft.activeSessionId;
              const isSameSession = receivedSessionId && currentSessionId === receivedSessionId;

              if (agentDraft.messageSource.mode !== "live" && !isSameSession) {
                agentDraft.messageSource = { mode: "live", messages: [] };
              }

              // Cast to any[] — sync status uses "streaming"/"complete" values not in ChatMessage.status union
              const liveMessages = (agentDraft.messageSource as any).messages as any[];
              const existingIdx = liveMessages.findIndex((m: any) => m.id === assistantId);

              if (existingIdx >= 0) {
                const existingMsg = liveMessages[existingIdx];
                // Merge content: keep local text if it's ahead of sync (prevents flicker)
                // but accept sync if it's significantly different (recon from scratch)
                const localTextLen = (existingMsg.parts as MessagePart[])
                  .filter((p: MessagePart): p is TextPart => p.type === "text")
                  .reduce((acc: number, p: TextPart) => acc + (p.text?.length ?? 0), 0);
                const syncTextLen = syncParts
                  .filter((p: MessagePart): p is TextPart => p.type === "text")
                  .reduce((acc: number, p: TextPart) => acc + (p.text?.length ?? 0), 0);

                if (syncTextLen > localTextLen || Math.abs(syncTextLen - localTextLen) > 50) {
                   existingMsg.parts = syncParts;
                }

                if (existingMsg.status !== "complete") {
                  existingMsg.status = (syncStatus === "done" || syncStatus === "finished") ? "complete" : "streaming";
                }
              } else {
                liveMessages.push({
                  id: assistantId,
                  role: "assistant",
                  parts: syncParts,
                  createdAt: assistantCreatedAt,
                  agentId: currentRespondingAgent ?? undefined,
                  status: (syncStatus === "done" || syncStatus === "finished") ? "complete" : "streaming",
                });
              }

              if (agentDraft.connectionPhase !== "error" && syncStatus !== "done" && syncStatus !== "finished") {
                agentDraft.connectionPhase = "streaming";
              } else if (syncStatus === "done" || syncStatus === "finished") {
                agentDraft.connectionPhase = "idle";
              }
            });

            if (syncStatus === "finished" || syncStatus === "error" || syncStatus === "interrupted") {
              const errorText = syncStatus === "error" ? (event.error ?? null) : null;
              const newPhase: ConnectionPhase = syncStatus === "error" ? "error" : "idle";
              session.write({
                streamError: errorText,
                connectionPhase: newPhase,
                connectionError: errorText,
              });
            }
            break;
          }

          case "finish": {
            receivedFinishEvent = true;
            cancelScheduledUpdate();
            flushText();
            pushUpdate();
            incrementalParser.reset();
            assistantId = uuid();
            assistantCreatedAt = new Date().toISOString();
            parts = [];

            // Mark session as no longer running (don't rely solely on WS agent_processing event)
            if (receivedSessionId) {
              const sid = receivedSessionId;
              session.writeDraft((agentDraft: AgentState) => {
                agentDraft.activeSessionIds = (agentDraft.activeSessionIds || []).filter((id: string) => id !== sid);
              });
            }
            break;
          }

          case "error": {
            const errText = event.errorText;
            if (errText.includes("turn limit") || errText.includes("cycle detected")) {
              session.write({ turnLimitMessage: errText });
            } else {
              session.write({
                streamError: errText,
                connectionPhase: "error",
                connectionError: errText,
              });
            }
            break;
          }
        }
      }
    }
  } finally {
    reader.releaseLock();
    if (updateScheduled) {
      cancelScheduledUpdate();
      pushUpdate();
    }
    flushText();
    if (!session.signal.aborted) {
      if (parts.length > 0) pushUpdate();

      // SSE-02: Detect connection drop (stream ended without finish event).
      const agentState = callbacks.getAgentState(agent);
      const isError = agentState?.connectionPhase === "error";
      const effectiveSessionId = receivedSessionId ?? agentState?.activeSessionId;
      if (!isError && !receivedFinishEvent && effectiveSessionId) {
        callbacks.onReconnectNeeded(effectiveSessionId, reconnectAttempt);
        return;
      }

      if (!isError) {
        session.write({
          connectionPhase: "idle",
          connectionError: null,
          reconnectAttempt: 0,
        });
      }
      callbacks.onStreamDone?.();
    } else if (parts.length > 0) {
      const st = callbacks.getAgentState(agent);
      // Only persist partial text if this stream is still the current one.
      // After `abortLocalOnly` (navigation) bumps streamGeneration, this
      // write must NOT overwrite `messageSource` — the user is looking
      // at a different session now and a live-mode overlay from the
      // previous stream would visually clobber it. The corresponding
      // `receivedSessionId` guard in the sync-event path at the top of
      // this function already handles the mid-stream case; this branch
      // runs on clean abort with buffered parts, so it needs its own
      // generation check.
      if (
        st &&
        session.isCurrent &&
        (!receivedSessionId ||
          !st.activeSessionId ||
          st.activeSessionId === receivedSessionId)
      ) {
        const assistantMsg: ChatMessage = {
          id: assistantId,
          role: "assistant",
          parts: [...parts],
          createdAt: assistantCreatedAt,
          agentId: currentRespondingAgent ?? undefined,
        };
        const currentMessages = getLiveMessages(st.messageSource);
        const existing = currentMessages.findIndex((m: ChatMessage) => m.id === assistantId);
        const updated =
          existing >= 0
            ? currentMessages.map((m: ChatMessage, i: number) => (i === existing ? assistantMsg : m))
            : [...currentMessages, assistantMsg];
        session.write({ messageSource: { mode: "live", messages: updated } });
      }
    }
  }

  // Save and invalidate React Query caches, switch to history mode
  if (!session.signal.aborted) {
    if (receivedSessionId) {
      callbacks.onSessionId(receivedSessionId);
    }
    queryClient.invalidateQueries({ queryKey: qk.sessions(agent) });
    const completedSessionId = receivedSessionId ?? callbacks.getAgentState(agent)?.activeSessionId;
    if (completedSessionId) {
      queryClient.invalidateQueries({ queryKey: qk.sessionMessages(completedSessionId) });
      // Switch to history mode so UI renders from DB rows via convertHistory —
      // identical to what the user sees after F5 reload.
      session.write({ messageSource: { mode: "history", sessionId: completedSessionId } });
    }
  }
}

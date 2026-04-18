"use client";

import React, { useRef, useState, useEffect, useCallback, useMemo, type ReactNode } from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
import { useChatStore } from "@/stores/chat-store";
import type { ChatMessage } from "@/stores/chat-store";
import { Button } from "@/components/ui/button";
import { BarsLoader } from "@/components/ui/loader";
import { RoleAvatar } from "./ChatThread";

import { MessageItem } from "./MessageItem";
import { runScrollToBottom } from "./scroll-to-bottom";
import { attachTailSentinel } from "./tail-sentinel";
import { AgentTransitionDivider } from "@/components/chat/AgentTransitionDivider";
import { useAuthStore } from "@/stores/auth-store";
import { useSessions } from "@/lib/queries";
import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";
import { useTranslation } from "@/hooks/use-translation";

// ── Animation suppression ──────────────────────────────────────────────────

/**
 * Stream-aware animation gating:
 * - During active streaming (isStreaming=true): NO entrance animations on any message
 * - On history load: NO animations (messages are not "new")
 * - After stream completes: only very recent messages get a brief entrance animation
 *   (detected by: message was created within 2s AND streaming just stopped)
 */
function isNewMessage(msg: ChatMessage): boolean {
  if (!msg.createdAt) return false;
  return Date.now() - new Date(msg.createdAt).getTime() < 2000;
}

// ── Loading skeletons ──────────────────────────────────────────────────────

export function MessageSkeleton() {
  return (
    <div className="flex gap-3 py-5 md:py-6">
      <div className="h-9 w-9 rounded-xl bg-muted/50 animate-pulse shrink-0" />
      <div className="flex-1 space-y-2">
        <div className="h-3 w-20 rounded bg-muted/50 animate-pulse" />
        <div className="h-4 w-full rounded bg-muted/40 animate-pulse" />
        <div className="h-4 w-3/4 rounded bg-muted/30 animate-pulse" />
      </div>
    </div>
  );
}

function MessageListSkeleton() {
  return (
    <div className="mx-auto w-full max-w-4xl px-3 md:px-6 space-y-2">
      {[1, 2, 3, 4].map((i) => (
        <MessageSkeleton key={i} />
      ))}
    </div>
  );
}

// ── Thinking indicator ──────────────────────────────────────────────────────

function ThinkingMessage() {
  const currentAgent = useChatStore((s) => s.currentAgent);
  const displayAgent = currentAgent;
  const agentIcons = useAuthStore((s) => s.agentIcons);
  const agentIconUrl = displayAgent && agentIcons[displayAgent] ? `/uploads/${agentIcons[displayAgent]}` : null;

  return (
    <div className="flex gap-3 py-5 md:py-6 border-t border-border/30 dark:border-border/20 animate-in fade-in slide-in-from-bottom-2 duration-300 ease-out">
      <span className="message-avatar">
        <RoleAvatar role="assistant" iconUrl={agentIconUrl} agentName={displayAgent} />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="message-header min-h-[18px] flex items-center">
          <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground/70">
            {displayAgent}
          </span>
        </div>
        <BarsLoader size="sm" className="text-muted-foreground/40 pt-0.5" />
      </div>
    </div>
  );
}

// ── Scroll-to-bottom button ─────────────────────────────────────────────────

function ScrollToBottomButton({
  visible,
  isStreaming,
  newTokenCount,
  onClick,
  ariaLabel,
}: {
  /** Driven by `!isAtTail` — sentinel IO geometrically confirms the
   * viewport has left the tail. Clears automatically when the user
   * scrolls back to the bottom (IO fires true). */
  visible: boolean;
  isStreaming: boolean;
  newTokenCount: number;
  onClick: () => void;
  ariaLabel: string;
}) {
  if (!visible) return null;

  const badge = newTokenCount > 99 ? "99+" : newTokenCount > 0 ? String(newTokenCount) : null;

  return (
    <Button
      variant="outline"
      size="icon-lg"
      onClick={onClick}
      aria-label={ariaLabel}
      className="absolute bottom-4 right-6 z-10 rounded-full shadow-lg transition-all duration-150 ease-out"
    >
      <ChevronDown className="h-5 w-5" />
      {isStreaming && (
        <span className="absolute -top-1 -right-1 h-3 w-3 rounded-full bg-primary animate-pulse" />
      )}
      {badge && (
        <span className="absolute -bottom-1 -right-1 min-w-[18px] h-[18px] rounded-full bg-primary text-primary-foreground text-[10px] font-bold flex items-center justify-center px-1 leading-none">
          {badge}
        </span>
      )}
    </Button>
  );
}

// ── Virtuoso Header / Footer ───────────────────────────────────────────────

function VirtuosoHeader({ hiddenCount, onLoadEarlier }: { hiddenCount: number; onLoadEarlier: () => void }) {
  const { t } = useTranslation();
  if (hiddenCount <= 0) return null;
  return (
    <div className="flex items-center justify-center py-4">
      <button
        onClick={onLoadEarlier}
        className="text-xs text-muted-foreground/60 hover:text-muted-foreground transition-colors border border-border/40 rounded-full px-4 py-1.5 hover:bg-muted/30"
      >
        {t("chat.show_earlier", { count: hiddenCount })}
      </button>
    </div>
  );
}

function VirtuosoFooter({ turnLimitMessage }: { turnLimitMessage: string | null }) {
  return (
    <div className="mx-auto w-full max-w-4xl px-3 md:px-6 pb-2">
      {turnLimitMessage && (
        <div
          data-testid="turn-limit-message"
          className="flex items-center gap-3 rounded-lg border border-amber-500/30 bg-amber-500/5 dark:bg-amber-500/10 px-4 py-3 text-sm text-amber-700 dark:text-amber-400 my-3 animate-in fade-in slide-in-from-bottom-2 duration-200"
        >
          <svg className="h-4 w-4 shrink-0" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m9-.75a9 9 0 1 1-18 0 9 9 0 0 1 18 0Zm-9 3.75h.008v.008H12v-.008Z" />
          </svg>
          <span>{turnLimitMessage}</span>
        </div>
      )}
    </div>
  );
}

// ── Turn limit selector ───────────────────────────────────────────────────

function useTurnLimitMessage() {
  return useChatStore((s) => s.agents[s.currentAgent]?.turnLimitMessage ?? null);
}

// ── Main MessageList component ──────────────────────────────────────────────

export function MessageList({
  messages,
  isStreaming,
  showThinking,
  isLoadingHistory,
  emptyState,
  hiddenCount,
  onLoadEarlier,
}: {
  messages: ChatMessage[];
  isStreaming: boolean;
  showThinking: boolean;
  isLoadingHistory: boolean;
  emptyState: ReactNode;
  hiddenCount: number;
  onLoadEarlier: () => void;
}) {
  const { t } = useTranslation();
  const turnLimitMessage = useTurnLimitMessage();
  const virtuosoRef = useRef<VirtuosoHandle>(null);

  // ── Tail-sentinel auto-follow ──────────────────────────────────────────────
  // Single source of truth for "is the viewport at the tail?".
  // Driven exclusively by IntersectionObserver on a 1 px sentinel in
  // Virtuoso's Footer. See `./tail-sentinel.ts` and
  // 2026-04-18-tail-sentinel-auto-follow-design.md.
  const [isAtTail, setIsAtTail] = useState<boolean>(true);
  /** Ref mirror: Virtuoso's `followOutput` callback is invoked outside
   * React's render cycle and must read the most recent value
   * synchronously. */
  const isAtTailRef = useRef<boolean>(true);
  /** DOM node of the sentinel element, exposed as state so the IO
   * effect re-attaches whenever Virtuoso remounts its Footer
   * (e.g., on parent re-render with unstable `components` memo
   * inputs). Using a ref alone would leave the IntersectionObserver
   * bound to a detached element. */
  const [sentinelEl, setSentinelEl] = useState<HTMLDivElement | null>(null);

  // Track tokens received while auto-follow is off (SCRL-03).
  const [missedTokens, setMissedTokens] = useState(0);
  const missedTokensRef = useRef(0); // shadow ref to avoid stale closure in effect

  // Hoist session data so individual UserMessage components don't each subscribe
  const currentAgent = useChatStore((s) => s.currentAgent);
  const activeSessionId = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionId ?? null);
  const { data: sessionsData } = useSessions(currentAgent ?? "");
  const activeSession = sessionsData?.sessions.find((s) => s.id === activeSessionId);
  const sessionChannel = activeSession?.channel;
  const sessionUserId = activeSession?.user_id;

  // Append a virtual "thinking" item so Virtuoso's followOutput auto-scrolls to it.
  // This is more reliable than rendering in Footer (which is outside the item list).
  // Must be defined before effects that reference virtualItems.length.
  const THINKING_ID = "__thinking__";
  const virtualItems = useMemo(() => {
    if (!showThinking) return messages;
    const thinkingItem: ChatMessage = {
      id: THINKING_ID,
      role: "assistant" as const,
      parts: [],
      createdAt: new Date().toISOString(),
    };
    return [...messages, thinkingItem];
  }, [messages, showThinking]);

  // ── IntersectionObserver: sentinel → isAtTail ──────────────────────────────
  // Watches a 1 px sentinel inside Virtuoso's Footer. Replaces the
  // previous ResizeObserver + input-event detectors, which were all
  // derived from scrollTop and broke under Virtuoso's virtualization.
  //
  // Effect depends on `sentinelEl` (not just a ref) so that if
  // Virtuoso remounts the Footer — which happens whenever parent
  // props passed into `virtuosoComponents` are unstable — the new
  // DOM node triggers a re-attach of the IntersectionObserver.
  const scrollContainerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const container = scrollContainerRef.current;
    if (!container || !sentinelEl) return;

    let cancelled = false;
    let detach: (() => void) | null = null;
    let attempts = 0;

    const tryAttach = () => {
      if (cancelled) return;
      const scroller = container.querySelector(
        "[data-virtuoso-scroller]",
      ) as HTMLElement | null;
      if (!scroller) {
        if (attempts++ < 10) {
          requestAnimationFrame(tryAttach);
        } else {
          // eslint-disable-next-line no-console
          console.warn(
            "tail-sentinel: scroller not found after 10 frames",
          );
        }
        return;
      }
      detach = attachTailSentinel(scroller, sentinelEl, (atTail) => {
        isAtTailRef.current = atTail;
        setIsAtTail((prev) => (prev === atTail ? prev : atTail));
      });
    };
    tryAttach();

    return () => {
      cancelled = true;
      detach?.();
    };
  }, [sentinelEl]);

  // Reset the missed-token badge when the user re-enters the tail.
  // Separated from the IO callback to keep the `setIsAtTail` updater
  // pure (React 19's strict-mode purity check flags nested setState
  // calls from inside another updater).
  useEffect(() => {
    if (isAtTail) {
      missedTokensRef.current = 0;
      setMissedTokens(0);
    }
  }, [isAtTail]);

  // ── SCRL-03: count tokens that arrived while user is away from tail ────────
  const prevPartsLenRef = useRef(0);
  useEffect(() => {
    const totalParts = virtualItems.reduce((acc, m) => acc + m.parts.length, 0);
    if (totalParts > prevPartsLenRef.current && !isAtTailRef.current) {
      const delta = totalParts - prevPartsLenRef.current;
      missedTokensRef.current += delta;
      setMissedTokens(missedTokensRef.current);
    }
    prevPartsLenRef.current = totalParts;
  }, [virtualItems]);

  // Force scroll to bottom on session switch. Sentinel IO will flip
  // isAtTail back to true once the scroll settles.
  const prevSessionIdRef = useRef(activeSessionId);
  useEffect(() => {
    if (prevSessionIdRef.current !== activeSessionId) {
      prevSessionIdRef.current = activeSessionId;
      const t = setTimeout(
        () => virtuosoRef.current?.scrollTo({ top: Number.MAX_SAFE_INTEGER, behavior: "smooth" }),
        200,
      );
      return () => clearTimeout(t);
    }
  }, [activeSessionId]);

  // Force scroll when stream starts (user submitted a message).
  // Sentinel IO will flip isAtTail back to true once the scroll
  // settles — no manual state dispatch needed.
  const prevStreamingRef = useRef(isStreaming);
  useEffect(() => {
    const streamJustStarted = !prevStreamingRef.current && isStreaming;
    prevStreamingRef.current = isStreaming;
    if (streamJustStarted && virtualItems.length > 0) {
      missedTokensRef.current = 0;
      setMissedTokens(0);
      virtuosoRef.current?.scrollToIndex({
        index: virtualItems.length - 1,
        align: "end",
        behavior: "smooth",
      });
    }
  }, [isStreaming, virtualItems.length]);

  const scrollToBottom = useCallback(() => {
    runScrollToBottom(virtuosoRef.current);
    // Sentinel IO will flip isAtTail to true on settle. Reset the
    // badge immediately so the UI feels responsive.
    missedTokensRef.current = 0;
    setMissedTokens(0);
  }, []);

  const virtuosoComponents = useMemo(() => ({
    Header: () => <VirtuosoHeader hiddenCount={hiddenCount} onLoadEarlier={onLoadEarlier} />,
    Footer: () => (
      <>
        <VirtuosoFooter turnLimitMessage={turnLimitMessage} />
        <div
          ref={setSentinelEl}
          aria-hidden="true"
          data-testid="tail-sentinel"
          style={{ height: 1, width: "100%" }}
        />
      </>
    ),
  }), [hiddenCount, onLoadEarlier, turnLimitMessage]);

  // Loading state — show skeletons while history is being fetched
  if (isLoadingHistory && messages.length === 0) {
    return (
      <div className="flex flex-1 flex-col overflow-y-auto pt-14 lg:pt-0">
        <MessageListSkeleton />
      </div>
    );
  }

  // Empty state
  if (messages.length === 0 && !showThinking) {
    return (
      <div className="flex flex-1 flex-col overflow-y-auto pt-14 lg:pt-0">
        {emptyState}
      </div>
    );
  }

  return (
    <div ref={scrollContainerRef} className="flex flex-1 flex-col pt-14 lg:pt-0 relative">
      <Virtuoso
        ref={virtuosoRef}
        data={virtualItems}
        computeItemKey={(index, item) => item.id}
        defaultItemHeight={120}
        alignToBottom
        skipAnimationFrameInResizeObserver
        followOutput={() => (isAtTailRef.current ? "auto" : false)}
        atBottomThreshold={100}
        initialTopMostItemIndex={messages.length > 0 ? messages.length - 1 : 0}
        increaseViewportBy={{ top: 500, bottom: 200 }}
        components={virtuosoComponents}
        itemContent={(index, msg) => {
          // Virtual thinking item — render the thinking indicator as a data item
          // so Virtuoso's followOutput auto-scrolls to it
          if (msg.id === THINKING_ID) {
            return (
              <div className="mx-auto w-full max-w-4xl px-3 md:px-6 py-2">
                <ThinkingMessage />
              </div>
            );
          }

          const prev = index > 0 ? virtualItems[index - 1] : null;
          const showSeparator =
            prev !== null &&
            prev.id !== THINKING_ID &&
            prev.role === "assistant" &&
            msg.role === "assistant" &&
            !!prev.agentId && !!msg.agentId &&
            prev.agentId !== msg.agentId &&
            prev.agentId != null &&
            msg.agentId != null;

          // Only animate messages that arrived AFTER streaming stopped and are very recent.
          // ALSO animate the first message of a new agent after an agent switch.
          const isNew = (!isStreaming && isNewMessage(msg)) || (showSeparator && isStreaming);

          return (
            <div className="mx-auto w-full max-w-4xl px-3 md:px-6">
              {showSeparator && (
                <AgentTransitionDivider agentName={msg.agentId!} />
              )}
              <div className={cn(
                isNew && "animate-in fade-in slide-in-from-bottom-2 duration-300 ease-out",
                isStreaming && index === virtualItems.length - 1 && msg.role === "assistant" && "streaming-message",
              )}>
                <MessageItem message={msg} sessionChannel={sessionChannel} sessionUserId={sessionUserId} />
              </div>
            </div>
          );
        }}
      />

      <ScrollToBottomButton
        visible={!isAtTail}
        isStreaming={isStreaming}
        newTokenCount={missedTokens}
        onClick={scrollToBottom}
        ariaLabel={t("chat.scroll_to_bottom")}
      />
    </div>
  );
}

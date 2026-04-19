"use client";

import { useState, useRef, useEffect, useCallback } from "react";
import { type VirtuosoHandle } from "react-virtuoso";
import { attachTailSentinel } from "./tail-sentinel";
import { runScrollToBottom } from "./scroll-to-bottom";

/**
 * Encapsulated auto-follow and tail-detection logic for the chat message list.
 * Fixes performance (O(1) token tracking), rAF conflicts, and inertia-lock bugs.
 */
export function useChatAutoscroll(
  isStreaming: boolean,
  activeSessionId: string | null
) {
  const virtuosoRef = useRef<VirtuosoHandle>(null);

  // Geometric state: is the viewport physically at the bottom?
  const [isAtTail, setIsAtTail] = useState(true);
  const isAtTailRef = useRef(true);

  // Intent state: should the viewport follow new content?
  const [shouldFollow, setShouldFollow] = useState(true);
  const shouldFollowRef = useRef(true);

  // Badge state: how many tokens arrived while user was away?
  const [missedTokens, setMissedTokens] = useState(0);
  const missedTokensRef = useRef(0);

  // DOM elements (exposed via state to trigger effect re-attach on remount)
  const [sentinelEl, setSentinelEl] = useState<HTMLDivElement | null>(null);
  const [scrollerEl, setScrollerEl] = useState<HTMLElement | null>(null);

  // Performance: track token growth for the tail message ONLY (O(1))
  const lastMsgPartsLenRef = useRef(0);
  const lastMsgIdRef = useRef<string | null>(null);

  // Reset badge when user reaches the tail
  useEffect(() => {
    if (isAtTail) {
      missedTokensRef.current = 0;
      setMissedTokens(0);
    }
  }, [isAtTail]);

  // IntersectionObserver: sentinel (in Footer) -> isAtTail
  useEffect(() => {
    if (!scrollerEl || !sentinelEl) return;

    return attachTailSentinel(scrollerEl, sentinelEl, (atTail) => {
      isAtTailRef.current = atTail;
      setIsAtTail(atTail);

      // Auto-restore follow intent when user manually reaches the bottom
      if (atTail && !shouldFollowRef.current) {
        shouldFollowRef.current = true;
        setShouldFollow(true);
      }
    });
  }, [scrollerEl, sentinelEl]);

  // Gentle rAF pin: ensures we stay at the bottom during rapid streaming/layout shifts.
  // Only active when shouldFollow is TRUE and streaming is ACTIVE.
  useEffect(() => {
    if (!scrollerEl || !isStreaming) return;

    let raf = 0;
    const loop = () => {
      if (shouldFollowRef.current && scrollerEl) {
        const maxScroll = scrollerEl.scrollHeight - scrollerEl.clientHeight;
        // Only force if we drifted more than a few pixels (avoids jittering with Virtuoso's internal scroll)
        if (scrollerEl.scrollTop < maxScroll - 5) {
          scrollerEl.scrollTop = scrollerEl.scrollHeight;
        }
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);

    return () => cancelAnimationFrame(raf);
  }, [scrollerEl, isStreaming]);

  // User-intent detection: wheel/touch/key events flip shouldFollow OFF.
  useEffect(() => {
    if (!scrollerEl) return;
    let touchStartY = 0;

    const turnOff = () => {
      if (shouldFollowRef.current) {
        shouldFollowRef.current = false;
        setShouldFollow(false);
      }
    };

    const onWheel = (e: WheelEvent) => {
      // Threshold -20 avoids small jitters/inertia tails from killing follow intent.
      if (e.deltaY < -20) turnOff();
    };
    const onTouchStart = (e: TouchEvent) => {
      touchStartY = e.touches[0]?.clientY ?? 0;
    };
    const onTouchMove = (e: TouchEvent) => {
      const dy = (e.touches[0]?.clientY ?? 0) - touchStartY;
      if (dy > 25) turnOff(); // User pulled content DOWN (scrolled UP)
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (["PageUp", "ArrowUp", "Home"].includes(e.key)) turnOff();
    };

    scrollerEl.addEventListener("wheel", onWheel, { passive: true });
    scrollerEl.addEventListener("touchstart", onTouchStart, { passive: true });
    scrollerEl.addEventListener("touchmove", onTouchMove, { passive: true });
    scrollerEl.addEventListener("keydown", onKeyDown);

    return () => {
      scrollerEl.removeEventListener("wheel", onWheel);
      scrollerEl.removeEventListener("touchstart", onTouchStart);
      scrollerEl.removeEventListener("touchmove", onTouchMove);
      scrollerEl.removeEventListener("keydown", onKeyDown);
    };
  }, [scrollerEl]);

  // Reset follow intent on session switch
  const prevSessionId = useRef(activeSessionId);
  useEffect(() => {
    if (activeSessionId !== prevSessionId.current) {
      prevSessionId.current = activeSessionId;
      shouldFollowRef.current = true;
      setShouldFollow(true);
      // Wait for session history to load/render before anchoring
      const t = setTimeout(() => {
        if (scrollerEl) scrollerEl.scrollTop = scrollerEl.scrollHeight;
      }, 100);
      return () => clearTimeout(t);
    }
  }, [activeSessionId, scrollerEl]);

  const scrollToBottom = useCallback(() => {
    shouldFollowRef.current = true;
    setShouldFollow(true);
    runScrollToBottom(virtuosoRef.current);
    // UI response: clear badge immediately
    setMissedTokens(0);
    missedTokensRef.current = 0;
  }, []);

  const trackNewTokens = useCallback((lastMsgId: string, partsCount: number) => {
    if (!isAtTailRef.current) {
      if (lastMsgId === lastMsgIdRef.current) {
        const delta = Math.max(0, partsCount - lastMsgPartsLenRef.current);
        missedTokensRef.current += delta;
      } else {
        // New message started while away from tail
        missedTokensRef.current += partsCount;
      }
      setMissedTokens(missedTokensRef.current);
    }
    lastMsgIdRef.current = lastMsgId;
    lastMsgPartsLenRef.current = partsCount;
  }, []);

  return {
    virtuosoRef,
    setSentinelEl,
    setScrollerEl,
    isAtTail,
    shouldFollow,
    missedTokens,
    scrollToBottom,
    trackNewTokens,
  };
}

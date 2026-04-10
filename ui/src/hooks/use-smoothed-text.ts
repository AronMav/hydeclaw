"use client";

import { useRef, useState, useEffect, useCallback } from "react";

/**
 * CharacterInterpolator hook — smoothly "prints" text at a constant rate,
 * absorbing network jitter from SSE streaming.
 *
 * @param rawText - The full text received so far from the stream
 * @param isStreaming - Whether the stream is still active
 * @param charsPerFrame - Characters to reveal per animation frame (~60fps)
 * @returns The smoothed text to display
 */
export function useSmoothedText(
  rawText: string,
  isStreaming: boolean,
  charsPerFrame = 3,
): string {
  const [displayed, setDisplayed] = useState("");
  const targetRef = useRef(rawText);
  const displayedRef = useRef("");
  const rafRef = useRef<number | null>(null);

  targetRef.current = rawText;

  const tick = useCallback(() => {
    const target = targetRef.current;
    const current = displayedRef.current;

    if (current.length < target.length) {
      // Reveal next chunk of characters
      const nextLen = Math.min(current.length + charsPerFrame, target.length);
      const next = target.slice(0, nextLen);
      displayedRef.current = next;
      setDisplayed(next);
      rafRef.current = requestAnimationFrame(tick);
    } else {
      rafRef.current = null;
    }
  }, [charsPerFrame]);

  useEffect(() => {
    // When raw text grows, start the animation loop
    if (rawText.length > displayedRef.current.length && !rafRef.current) {
      rafRef.current = requestAnimationFrame(tick);
    }
  }, [rawText, tick]);

  useEffect(() => {
    // When streaming ends, instantly show everything (no trailing delay)
    if (!isStreaming) {
      if (rafRef.current) {
        cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
      displayedRef.current = rawText;
      setDisplayed(rawText);
    }
  }, [isStreaming, rawText]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
  }, []);

  return isStreaming ? displayed : rawText;
}

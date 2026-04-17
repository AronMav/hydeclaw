"use client";

import { useRef, useState, useEffect } from "react";

/**
 * CharacterInterpolator — обеспечивает идеально плавный вывод текста.
 * Адаптирует скорость печати под размер буфера (чем больше текста накопилось, тем быстрее вывод).
 */
export function useSmoothedText(rawText: string, isStreaming: boolean) {
  const [displayedText, setDisplayValue] = useState(rawText);
  const queueRef = useRef("");
  const frameRef = useRef<number | null>(null);
  const streamingRef = useRef(isStreaming);
  streamingRef.current = isStreaming;
  
  // Синхронизируем очередь при получении новых данных
  useEffect(() => {
    if (!isStreaming) {
      setDisplayValue(rawText);
      queueRef.current = "";
      return;
    }

    // Если rawText стал короче (регенерация), сбрасываем всё
    if (rawText.length < displayedText.length) {
      setDisplayValue(rawText);
      queueRef.current = "";
      return;
    }

    queueRef.current = rawText.slice(displayedText.length);
  }, [rawText, isStreaming, displayedText.length]);

  useEffect(() => {
    if (!isStreaming && queueRef.current.length === 0) return;

    const animate = () => {
      if (queueRef.current.length > 0) {
        // Адаптивная скорость: минимум 1 символ, максимум 15% очереди за кадр
        const jump = Math.ceil(queueRef.current.length * 0.15);
        const charsToShow = Math.min(queueRef.current.length, jump);

        const nextPart = queueRef.current.slice(0, charsToShow);
        queueRef.current = queueRef.current.slice(charsToShow);

        setDisplayValue((prev) => prev + nextPart);
        frameRef.current = requestAnimationFrame(animate);
      } else if (streamingRef.current) {
        // Queue empty but still streaming — poll next frame
        frameRef.current = requestAnimationFrame(animate);
      }
      // Otherwise: queue empty + not streaming → stop loop
    };

    frameRef.current = requestAnimationFrame(animate);
    return () => {
      if (frameRef.current) cancelAnimationFrame(frameRef.current);
    };
  }, [isStreaming]);

  return displayedText;
}

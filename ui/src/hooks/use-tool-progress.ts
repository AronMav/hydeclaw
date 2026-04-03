import { useState, useEffect, useRef } from "react";

/**
 * Simulates tool execution progress: 0→95% gradually, then jumps to 100% on completion.
 * Never reaches 100% on its own — prevents false "done" signal before actual completion.
 * Based on LibreChat useProgress.ts pattern.
 */
export function useToolProgress(isRunning: boolean): number {
  const [progress, setProgress] = useState(0);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    if (isRunning) {
      setProgress(0.01);
      intervalRef.current = setInterval(() => {
        setProgress((prev) => {
          if (prev >= 0.95) {
            if (intervalRef.current) clearInterval(intervalRef.current);
            return 0.95;
          }
          // Non-linear growth: fast at start, slow as it approaches 95%
          const increment = prev < 0.5 ? 0.02 : prev < 0.8 ? 0.008 : 0.003;
          return Math.min(prev + increment, 0.95);
        });
      }, 200);
    } else {
      if (intervalRef.current) clearInterval(intervalRef.current);
      // Jump to 100% on completion
      setProgress((prev) => (prev > 0 ? 1 : 0));
    }

    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, [isRunning]);

  return progress;
}

"use client";

import { useEffect, useRef } from "react";

export function useAutoRefresh(callback: () => void, intervalMs: number) {
  const savedCallback = useRef(callback);

  useEffect(() => {
    savedCallback.current = callback;
  }, [callback]);

  useEffect(() => {
    const id = setInterval(() => {
      if (document.visibilityState !== "hidden") savedCallback.current();
    }, intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
}

"use client";

import { useEffect, useState } from "react";

/**
 * Returns the keyboard height in pixels (0 when keyboard is closed).
 * Uses the VisualViewport API to detect when the software keyboard
 * reduces the visible area. Falls back to 0 on unsupported browsers.
 */
export function useVisualViewport() {
  const [keyboardHeight, setKeyboardHeight] = useState(0);

  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return;

    function onResize() {
      // When keyboard is open, visualViewport.height < window.innerHeight
      const kbH = window.innerHeight - (vv?.height ?? window.innerHeight);
      setKeyboardHeight(kbH > 0 ? kbH : 0);
    }

    vv.addEventListener("resize", onResize);
    vv.addEventListener("scroll", onResize);
    return () => {
      vv.removeEventListener("resize", onResize);
      vv.removeEventListener("scroll", onResize);
    };
  }, []);

  return keyboardHeight;
}

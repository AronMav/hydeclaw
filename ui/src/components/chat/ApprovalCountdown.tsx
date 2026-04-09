"use client";

import { useState, useEffect, useRef } from "react";
import { useTranslation } from "@/hooks/use-translation";

interface ApprovalCountdownProps {
  timeoutMs: number;
  receivedAt: number;
  status: string;
}

function formatTime(ms: number): string {
  const totalSeconds = Math.ceil(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

export function ApprovalCountdown({ timeoutMs, receivedAt, status }: ApprovalCountdownProps) {
  const { t } = useTranslation();
  const [remaining, setRemaining] = useState(() =>
    Math.max(0, timeoutMs - (Date.now() - receivedAt)),
  );
  const lastAnnouncedRef = useRef<number>(remaining);
  const [ariaText, setAriaText] = useState("");

  useEffect(() => {
    if (status !== "pending") return;

    const interval = setInterval(() => {
      const next = Math.max(0, timeoutMs - (Date.now() - receivedAt));
      setRemaining(next);

      // Update aria text every 30 seconds to avoid screen reader noise
      if (Math.abs(lastAnnouncedRef.current - next) >= 30_000 || next <= 1000) {
        lastAnnouncedRef.current = next;
        setAriaText(t("chat.approval_countdown", { time: formatTime(next) }));
      }
    }, 1000);

    return () => clearInterval(interval);
  }, [timeoutMs, receivedAt, status, t]);

  if (status !== "pending") return null;

  const formatted = formatTime(remaining);
  const isUrgent = remaining < 30_000;

  return (
    <span className={`text-xs font-mono ${isUrgent ? "text-destructive" : "text-warning"}`}>
      <span aria-hidden="true">
        {t("chat.approval_countdown", { time: formatted })}
      </span>
      <span className="sr-only" aria-live="polite" aria-atomic="true">
        {ariaText}
      </span>
    </span>
  );
}

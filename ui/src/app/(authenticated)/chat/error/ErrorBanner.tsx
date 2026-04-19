"use client";

import { useState } from "react";
import { cn } from "@/lib/utils";
import { useTranslation } from "@/hooks/use-translation";
import { Button } from "@/components/ui/button";
import { WifiOff, Clock, AlertTriangle, RotateCcw, X } from "lucide-react";

// ── Error text (expandable) ──────────────────────────────────────────────────

function ErrorText({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <span
      className={cn("flex-1 cursor-pointer", !expanded && "line-clamp-2")}
      onClick={() => setExpanded(!expanded)}
    >
      {text}
    </span>
  );
}

// ── Error banner ─────────────────────────────────────────────────────────────

export type StreamErrorType = "connection_lost" | "timeout" | "api_error";

export function classifyStreamError(error: string): StreamErrorType {
  const lower = error.toLowerCase();
  if (
    lower.includes("connection lost") ||
    lower.includes("failed to fetch") ||
    lower.includes("network") ||
    lower.includes("disconnected") ||
    lower.includes("aborted")
  ) {
    return "connection_lost";
  }
  if (lower.includes("timeout") || lower.includes("timed out")) {
    return "timeout";
  }
  return "api_error";
}

export function ErrorBanner({
  error,
  hasMessages,
  onClear,
  onRetry,
}: {
  error: string;
  hasMessages: boolean;
  onClear: () => void;
  onRetry: () => void;
}) {
  const { t } = useTranslation();
  const errorType = classifyStreamError(error);

  const isAmber = errorType === "connection_lost" || errorType === "timeout";
  const containerClass = isAmber
    ? "border-amber-500/30 bg-amber-500/5 dark:bg-amber-500/15 text-amber-700 dark:text-amber-400"
    : "border-destructive/30 bg-destructive/5 dark:bg-destructive/15 text-destructive";

  const IconComponent =
    errorType === "connection_lost" ? WifiOff :
    errorType === "timeout" ? Clock :
    AlertTriangle;

  const label =
    errorType === "connection_lost" ? t("chat.error_connection_lost") :
    errorType === "timeout" ? t("chat.error_timeout") :
    t("chat.error_api");

  const retryLabel =
    errorType === "connection_lost" ? t("chat.error_reconnect") : t("error.retry");

  const buttonHoverClass = isAmber
    ? "hover:bg-amber-500/10 text-amber-700 dark:text-amber-400"
    : "text-destructive hover:bg-destructive/10";

  const closeHoverClass = isAmber
    ? "hover:bg-amber-500/20 text-amber-700/60 hover:text-amber-700 dark:text-amber-400/60 dark:hover:text-amber-400"
    : "hover:bg-destructive/20 text-destructive/60 hover:text-destructive";

  return (
    <div className="shrink-0 px-3 md:px-4 pb-1">
      <div
        data-testid="stream-error-banner"
        data-error-type={errorType}
        className={cn(
          "mx-auto max-w-4xl flex items-center gap-3 rounded-lg border px-4 py-2.5 text-sm font-medium",
          containerClass,
        )}
      >
        <IconComponent className="h-4 w-4 shrink-0" />
        <span className="shrink-0 font-semibold">{label}</span>
        <ErrorText text={error} />
        {hasMessages && (
          <Button
            variant="ghost"
            size="xs"
            className={cn("shrink-0", buttonHoverClass)}
            onClick={onRetry}
          >
            <RotateCcw className="h-3 w-3 mr-1" />
            {retryLabel}
          </Button>
        )}
        <button
          onClick={onClear}
          className={cn("shrink-0 rounded p-0.5 transition-colors", closeHoverClass)}
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

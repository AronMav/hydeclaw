"use client";

import { PulseDotLoader } from "@/components/ui/loader";
import { cn } from "@/lib/utils";

interface ReconnectingIndicatorProps {
  attempt: number;
  maxAttempts: number;
  className?: string;
}

export function ReconnectingIndicator({ attempt, maxAttempts, className }: ReconnectingIndicatorProps) {
  return (
    <div
      role="status"
      aria-live="polite"
      aria-label={`Reconnecting, attempt ${attempt} of ${maxAttempts}`}
      className={cn(
        "mx-auto flex max-w-fit items-center gap-2 rounded-lg border border-primary/20 bg-muted/40 px-3 py-2",
        className,
      )}
    >
      <PulseDotLoader size="sm" />
      <span className="text-sm text-muted-foreground">
        Reconnecting
        <span className="inline-flex">
          <span className="animate-[loading-dots_1.4s_infinite_0.2s]">.</span>
          <span className="animate-[loading-dots_1.4s_infinite_0.4s]">.</span>
          <span className="animate-[loading-dots_1.4s_infinite_0.6s]">.</span>
        </span>
      </span>
      <span className="text-xs text-muted-foreground">
        (attempt <span className="text-foreground">{attempt}</span>/{maxAttempts})
      </span>
    </div>
  );
}

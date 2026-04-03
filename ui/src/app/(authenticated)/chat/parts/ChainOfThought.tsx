"use client";

// ChainOfThought is currently dead code -- not imported by any component in the
// new custom rendering path (MessageItem dispatches reasoning and tool parts
// directly). Retained for potential future use.

import { useState } from "react";
import { useTranslation } from "@/hooks/use-translation";
import { ChevronRight } from "lucide-react";
import type { MessagePart, ToolPart } from "@/stores/chat-store";
import { ReasoningPart } from "./ReasoningPart";
import { ToolCallPartView } from "../ChatThread";

export function ChainOfThought({ parts }: { parts: MessagePart[] }) {
  const { t } = useTranslation();
  const [isOpen, setIsOpen] = useState(false);

  return (
    <div className="rounded-xl border border-border/60 bg-card/30 overflow-hidden">
      <button
        type="button"
        onClick={() => setIsOpen((v) => !v)}
        className="flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-muted/30"
      >
        <div className="h-2 w-2 rounded-full bg-primary/60 animate-pulse" />
        <span className="font-mono text-xs font-semibold uppercase tracking-wider text-primary/70">
          {t("chat.reasoning")}
        </span>
        <ChevronRight
          className={`ml-auto h-4 w-4 text-muted-foreground/40 transition-transform duration-300 ${isOpen ? "rotate-90" : ""}`}
        />
      </button>
      {isOpen && (
        <div className="border-t border-border/30 space-y-3 p-3">
          {parts.map((part, i) => {
            if (part.type === "reasoning") {
              return <ReasoningPart key={i} text={part.text} />;
            }
            if (part.type === "tool") {
              const tp = part as ToolPart;
              return (
                <ToolCallPartView
                  key={i}
                  toolName={tp.toolName}
                  args={tp.input as Record<string, unknown>}
                  result={tp.output}
                  status={{ type: tp.state === "output-available" ? "complete" : tp.state === "output-error" ? "error" : "running" }}
                />
              );
            }
            return null;
          })}
        </div>
      )}
    </div>
  );
}

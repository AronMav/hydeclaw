"use client";

import { useTranslation } from "@/hooks/use-translation";

export function AgentTurnSeparator({ data, animate = false, turnCount }: { data: { agentName: string; reason: string }; animate?: boolean; turnCount?: number }) {
  const { t } = useTranslation();
  return (
    <div
      data-testid="agent-turn-separator"
      className={`flex items-center justify-center gap-2 py-3 text-xs text-muted-foreground/50${
        animate ? " animate-in fade-in duration-200 ease-out" : ""
      }`}
    >
      <div className={`h-px flex-1 bg-border/30${animate ? " origin-left" : ""}`}
           style={animate ? { animation: "expand-from-center 200ms ease-out" } : undefined} />
      <span>{turnCount ? `${t("chat.turn_n", { n: turnCount })} \u2014 ` : ""}{t("chat.agent_responding", { agent: data.agentName })}</span>
      <div className={`h-px flex-1 bg-border/30${animate ? " origin-right" : ""}`}
           style={animate ? { animation: "expand-from-center 200ms ease-out" } : undefined} />
    </div>
  );
}

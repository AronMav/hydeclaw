"use client"

import React, { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";
import { TableCard, MetricCard } from "@/components/ui/rich-card";

// ── Card component type ────────────────────────────────────────────────────

export type CardComponent = React.ComponentType<{ data: Record<string, unknown> }>;

// ── Subagent complete card ─────────────────────────────────────────────────

function SubagentCompleteCard({ data }: { data: Record<string, unknown> }) {
  const status = data.status as string ?? "unknown";
  const agent = data.subagent_id as string ?? "subagent";
  const task = data.task_preview as string ?? "";
  const isOk = status === "completed";

  return (
    <details className={`rounded-lg border px-3 py-2 text-sm ${isOk ? "border-success/30 bg-success/5" : "border-destructive/30 bg-destructive/5"}`}>
      <summary className="flex items-center gap-2 cursor-pointer list-none [&::-webkit-details-marker]:hidden">
        <span className={`h-2 w-2 rounded-full shrink-0 ${isOk ? "bg-success" : "bg-destructive"}`} />
        <span className="font-medium">{agent}</span>
        <span className="text-muted-foreground">{status}</span>
      </summary>
      {task && <p className="mt-2 text-muted-foreground text-xs whitespace-pre-wrap border-t border-border/20 pt-2">{task}</p>}
    </details>
  );
}

// ── Registry ───────────────────────────────────────────────────────────────

export const CARD_REGISTRY = new Map<string, CardComponent>([
  ["table", TableCard],
  ["metric", MetricCard],
  // "subagent-complete" intentionally not registered — info already visible in subagent tool call
]);

// ── Error boundary ─────────────────────────────────────────────────────────

interface CardErrorBoundaryProps {
  resetKey: string;
  children: ReactNode;
}

interface CardErrorBoundaryState {
  hasError: boolean;
}

export class CardErrorBoundary extends Component<CardErrorBoundaryProps, CardErrorBoundaryState> {
  constructor(props: CardErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false };
  }

  static getDerivedStateFromError(): CardErrorBoundaryState {
    return { hasError: true };
  }

  componentDidCatch(error: Error, _info: ErrorInfo) {
    console.error("[CardErrorBoundary]", error.message);
  }

  componentDidUpdate(prevProps: CardErrorBoundaryProps) {
    if (prevProps.resetKey !== this.props.resetKey && this.state.hasError) {
      this.setState({ hasError: false });
    }
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          Card rendering error
        </div>
      );
    }
    return this.props.children;
  }
}

// ── Generative UI slot ─────────────────────────────────────────────────────

interface GenerativeUISlotProps {
  cardType: string;
  data: Record<string, unknown>;
}

// Card types to silently skip (info already shown elsewhere)
const HIDDEN_CARD_TYPES = new Set(["subagent-complete", "agent-turn"]);

export function GenerativeUISlot({ cardType, data }: GenerativeUISlotProps) {
  if (HIDDEN_CARD_TYPES.has(cardType)) return null;

  const CardComp = CARD_REGISTRY.get(cardType);

  return (
    <div style={{ contentVisibility: "auto", containIntrinsicSize: "0 200px" }}>
      {CardComp ? (
        <CardErrorBoundary resetKey={cardType}>
          <CardComp data={data} />
        </CardErrorBoundary>
      ) : (
        <pre className="rounded-lg border bg-muted/30 p-4 text-sm font-mono whitespace-pre-wrap overflow-auto">
          {JSON.stringify(data, null, 2)}
        </pre>
      )}
    </div>
  );
}

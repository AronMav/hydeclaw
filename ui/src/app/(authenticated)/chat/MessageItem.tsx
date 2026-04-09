"use client";

import React, { memo, type ReactNode } from "react";
import { useChatStore } from "@/stores/chat-store";
import { useAuthStore } from "@/stores/auth-store";
import { useTranslation } from "@/hooks/use-translation";
import type { ChatMessage, MessagePart, ToolPart, ToolPartState } from "@/stores/chat-store";
import { findSiblings, getCachedRawMessages } from "@/stores/chat-store";
import { formatMessageTime } from "@/lib/format";
import { BranchNavigator } from "./BranchNavigator";
import { cn } from "@/lib/utils";
import { AlertCircle, ChevronRight } from "lucide-react";
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from "@/components/ui/collapsible";
import { BarsLoader } from "@/components/ui/loader";
import { MessageActions } from "./MessageActions";
import { TextPart } from "./parts/TextPart";
import { ReasoningPart } from "./parts/ReasoningPart";
import {
  RoleAvatar,
  ToolCallPartView,
  FileDataPartView,
  SourceUrlDataPartView,
  RichCardDataPartView,
} from "./ChatThread";


// ── Parts render cache (PERF-03) ───────────────────────────────────────────
// Module-scope WeakMap: keys are ChatMessage object references.
// With PERF-02 in-place Immer mutation, only the currently-streaming message
// gets its parts updated — all other messages keep stable object references,
// so cache entries survive across renders. Entries are GC'd when ChatMessage
// objects leave scope.
const _partsRenderCache = new WeakMap<ChatMessage, ReactNode[]>();

// ── Tool grouping threshold ─────────────────────────────────────────────────
// Minimum consecutive tool calls required to collapse into a ToolCallGroup.
export const TOOL_GROUP_THRESHOLD = 3;

// ── Tool status mapping ─────────────────────────────────────────────────────

export function mapToolPartState(state: ToolPartState): "calling" | "running" | "complete" | "error" | "denied" {
  switch (state) {
    case "input-streaming":
      return "calling";
    case "input-available":
      return "running";
    case "output-available":
      return "complete";
    case "output-error":
      return "error";
    case "output-denied":
      return "denied";
  }
}

// ── Empty part view (loading indicator for empty assistant messages) ─────────

function EmptyPartView() {
  return <BarsLoader size="sm" className="text-muted-foreground/40 py-1" />;
}

// ── Part renderer dispatch ──────────────────────────────────────────────────

function renderPart(part: MessagePart, index: number) {
  switch (part.type) {
    case "text":
      return <TextPart key={index} text={part.text} />;
    case "reasoning":
      return <ReasoningPart key={index} text={part.text} />;
    case "tool": {
      return (
        <ToolCallPartView
          key={index}
          toolName={part.toolName}
          args={part.input}
          result={part.output}
          status={{ type: mapToolPartState(part.state) }}
        />
      );
    }
    case "file":
      return <FileDataPartView key={index} data={{ url: part.url, mediaType: part.mediaType }} />;
    case "source-url":
      return <SourceUrlDataPartView key={index} data={{ url: part.url, title: part.title }} />;
    case "rich-card":
      return <RichCardDataPartView key={index} data={{ cardType: part.cardType, ...part.data }} />;
    default:
      return null;
  }
}

// ── Tool call grouping ─────────────────────────────────────────────────────

function ToolCallGroup({ parts }: { parts: ToolPart[] }) {
  const { t } = useTranslation();

  const allComplete = parts.every((p) => p.state === "output-available");
  const hasError = parts.some(
    (p) => p.state === "output-error" || p.state === "output-denied"
  );
  const runningCount = parts.filter(
    (p) => p.state === "input-streaming" || p.state === "input-available"
  ).length;

  return (
    <Collapsible className="rounded-lg border border-border/50 bg-muted/10">
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className="flex w-full items-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground transition-colors group"
        >
          <ChevronRight
            className="h-4 w-4 shrink-0 transition-transform duration-200 group-data-[state=open]:rotate-90"
          />
          <span className="font-medium">
            {runningCount > 0
              ? t("chat.tools_running", { running: runningCount, total: parts.length })
              : hasError
                ? t("chat.tools_with_errors", { count: parts.length })
                : t("chat.tools_used", { count: parts.length })}
          </span>
          {allComplete && !hasError && (
            <span className="ml-auto text-xs text-success">{t("chat.tools_all_complete")}</span>
          )}
          {hasError && (
            <span className="ml-auto text-xs text-destructive">{t("chat.tools_has_errors")}</span>
          )}
        </button>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="border-t border-border/30 px-1 py-1 space-y-1">
          {parts.map((tp, i) => (
            <ToolCallPartView
              key={i}
              toolName={tp.toolName}
              args={tp.input}
              result={tp.output}
              status={{ type: mapToolPartState(tp.state) }}
            />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

function renderPartsWithGrouping(parts: MessagePart[]) {
  const result: ReactNode[] = [];

  // Stage 2 & 3 Fix: Pre-filter empty/whitespace-only text parts that could break tool grouping
  const effectiveParts = parts.filter(p => {
    if (p.type === "text") return p.text.trim().length > 0;
    return true;
  });

  let i = 0;
  while (i < effectiveParts.length) {
    const part = effectiveParts[i];

    if (part.type === "tool") {
      // Collect consecutive tool parts
      const toolRun: ToolPart[] = [];
      while (i < effectiveParts.length && effectiveParts[i].type === "tool") {
        const p = effectiveParts[i];
        if (p.type === "tool") toolRun.push(p);
        i++;
      }

      if (toolRun.length >= TOOL_GROUP_THRESHOLD) {
        // Group 3+ consecutive tool calls
        result.push(<ToolCallGroup key={`tool-group-${i}`} parts={toolRun} />);
      } else {
        // Render individually (1-2 tool calls)
        toolRun.forEach((tp, j) => {
          result.push(
            <ToolCallPartView
              key={`tool-${i - toolRun.length + j}`}
              toolName={tp.toolName}
              args={tp.input}
              result={tp.output}
              status={{ type: mapToolPartState(tp.state) }}
            />
          );
        });
      }
    } else {
      result.push(renderPart(part, i));
      i++;
    }
  }

  return result;
}

// ── User message ────────────────────────────────────────────────────────────

function UserMessage({ message, sessionChannel, sessionUserId }: { message: ChatMessage; sessionChannel?: string; sessionUserId?: string }) {
  const { t, locale } = useTranslation();
  const agentIcons = useAuthStore((s) => s.agentIcons);
  const activeSessionId = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionId ?? null);

  // Compute branch siblings for this user message (only when branching data exists)
  const branchInfo = React.useMemo(() => {
    if (!message.parentMessageId || !activeSessionId) return null;
    const allRows = getCachedRawMessages(activeSessionId);
    if (allRows.length === 0) return null;
    const { siblings, index } = findSiblings(allRows, message.id);
    if (siblings.length <= 1) return null;
    return { parentMessageId: message.parentMessageId, siblings, index };
  }, [message.id, message.parentMessageId, activeSessionId]);

  const isReadOnly = sessionChannel === "heartbeat" || sessionChannel === "cron" || sessionChannel === "inter-agent";

  // Per-message agent sender via agentId prop (for inter-agent turn loop messages)
  const senderAgentName = message.agentId
    || (isReadOnly && sessionUserId?.startsWith("agent:") ? sessionUserId.slice(6) : null);
  const isAgentSender = !!senderAgentName;
  const senderIconUrl = senderAgentName && agentIcons[senderAgentName]
    ? `/uploads/${agentIcons[senderAgentName]}`
    : undefined;

  const isSending = message.status === "sending";
  const isFailed = message.status === "failed";

  return (
    <div
      data-role={isAgentSender ? "agent-sender" : "user"}
      className={cn(
        "group flex gap-3 py-5 md:py-6 border-t border-border/30 dark:border-border/20 first:border-t-0",
        isAgentSender && "bg-muted/20 dark:bg-muted/10 rounded-lg px-3",
        isFailed && "border-l-2 border-l-destructive pl-3"
      )}
    >
      <span className="message-avatar">
        <RoleAvatar
          role={isAgentSender ? "agent-sender" : "user"}
          iconUrl={isAgentSender ? senderIconUrl : undefined}
          agentName={isAgentSender ? senderAgentName : undefined}
        />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="message-header flex items-center justify-between min-h-[18px]">
          <div className="flex items-center gap-2">
            <span className={`text-xs font-semibold uppercase tracking-wider ${isAgentSender ? "text-muted-foreground/70" : "text-primary"}`}>
              {isAgentSender ? senderAgentName : t("chat.you")}
            </span>
            {message.createdAt && (
              <span className="text-[10px] font-mono tabular-nums text-muted-foreground/40 opacity-0 group-hover:opacity-100 transition-opacity">
                {formatMessageTime(message.createdAt, locale)}
              </span>
            )}
          </div>
          <div className="flex items-center gap-1">
            {branchInfo && (
              <BranchNavigator
                parentMessageId={branchInfo.parentMessageId}
                siblings={branchInfo.siblings}
                currentIndex={branchInfo.index}
              />
            )}
            <MessageActions message={message} showReload={false} />
          </div>
        </div>
        <div className={cn("min-w-0 space-y-3", isSending && "opacity-70")}>
          {message.parts.map((part, i) => renderPart(part, i))}
        </div>
        {isFailed && (
          <div className="flex items-center gap-2 mt-1 text-xs text-destructive">
            <AlertCircle className="h-3 w-3 shrink-0" />
            <span>{t("chat.failedToSend")}</span>
            <button
              type="button"
              className="underline hover:no-underline"
              onClick={() => useChatStore.getState().regenerate()}
            >
              {t("chat.retry")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

// ── Assistant message ───────────────────────────────────────────────────────

function AssistantMessage({ message }: { message: ChatMessage }) {
  const { t, locale } = useTranslation();
  const currentAgent = useChatStore((s) => s.currentAgent);
  const agentIcons = useAuthStore((s) => s.agentIcons);

  // Direct agentId from message props -- no more AgentTurnCounterContext hack
  const agentName = message.agentId || currentAgent;
  const agentIconUrl = agentName && agentIcons[agentName] ? `/uploads/${agentIcons[agentName]}` : null;

  const hasParts = message.parts.length > 0;

  // PERF-03: WeakMap cache for rendered parts — only re-render if message object changed.
  // Cache key is the ChatMessage object reference; PERF-02 in-place mutation ensures
  // non-streaming messages keep stable refs so they get cache hits across re-renders.
  let renderedParts: ReactNode[] | undefined;
  if (hasParts) {
    renderedParts = _partsRenderCache.get(message);
    if (!renderedParts) {
      renderedParts = renderPartsWithGrouping(message.parts);
      _partsRenderCache.set(message, renderedParts);
    }
  }

  return (
    <div data-role="assistant" className="group flex gap-3 py-5 md:py-6 border-t border-border/30 dark:border-border/20 first:border-t-0">
      <span className="message-avatar">
        <RoleAvatar role="assistant" iconUrl={agentIconUrl} agentName={agentName} />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="message-header flex items-center justify-between min-h-[18px]">
          <div className="flex items-center gap-2">
            <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground/70 truncate max-w-[120px]">
              {agentName || t("chat.assistant")}
            </span>
            {message.createdAt && (
              <span className="text-[10px] font-mono tabular-nums text-muted-foreground/40 opacity-0 group-hover:opacity-100 transition-opacity">
                {formatMessageTime(message.createdAt, locale)}
              </span>
            )}
          </div>
          <MessageActions message={message} showReload />
        </div>
        <div className="min-w-0 space-y-3">
          {hasParts ? renderedParts : <EmptyPartView />}
        </div>
      </div>
    </div>
  );
}

// ── Main MessageItem ────────────────────────────────────────────────────────

export const MessageItem = memo(function MessageItem({
  message,
  sessionChannel,
  sessionUserId,
}: {
  message: ChatMessage;
  sessionChannel?: string;
  sessionUserId?: string;
}) {
  if (message.role === "user") {
    return <UserMessage message={message} sessionChannel={sessionChannel} sessionUserId={sessionUserId} />;
  }
  return <AssistantMessage message={message} />;
});

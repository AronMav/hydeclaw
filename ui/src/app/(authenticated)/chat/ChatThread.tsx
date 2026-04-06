"use client";

import React, { Component, useState, useCallback, useRef, useEffect, useMemo } from "react";
import type { ErrorInfo, ReactNode } from "react";
import { cn } from "@/lib/utils";
import { getToken } from "@/lib/api";
import { useChatStore, isActiveStream, convertHistory } from "@/stores/chat-store";
import { sanitizeUrl } from "@/lib/sanitize-url";
import { useToolProgress } from "@/hooks/use-tool-progress";
import { useVisualViewport } from "@/hooks/use-visual-viewport";
import type { ChatMessage } from "@/stores/chat-store";
import { useTranslation } from "@/hooks/use-translation";
import { useAuthStore } from "@/stores/auth-store";
import { useSessionMessages, useAgents, useProviders, useProviderModels } from "@/lib/queries";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

import { truncateOutput } from "@/lib/format";
import type { SessionRow } from "@/types/api";

import { Collapsible, CollapsibleTrigger, CollapsibleContent } from "@/components/ui/collapsible";
import { BarsLoader } from "@/components/ui/loader";
import { Button } from "@/components/ui/button";
import { RichCard } from "@/components/ui/rich-card";
import { SlashMenu } from "./parts/SlashMenu";
import { MessageList } from "./MessageList";
import { Avatar, AvatarImage, AvatarFallback } from "@/components/ui/avatar";
import {
  Bot,
  Send,
  Square,
  ChevronRight,
  Download,
  Paperclip,
  User,
  RotateCcw,
  X,
} from "lucide-react";

const EMPTY_LIVE_MESSAGES: ChatMessage[] = [];

// ── Props ────────────────────────────────────────────────────────────────────

interface ChatThreadProps {
  streamError: string | null;
  isReadOnly: boolean;
  activeSession?: SessionRow;
  onClearError: () => void;
  onRetry: () => void;
}

// ── Avatar colors & hashing ──────────────────────────────────────────────────

export const AGENT_COLORS = [
  "bg-blue-500/15 text-blue-600 dark:text-blue-400 border-blue-500/25",
  "bg-purple-500/15 text-purple-600 dark:text-purple-400 border-purple-500/25",
  "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400 border-emerald-500/25",
  "bg-amber-500/15 text-amber-600 dark:text-amber-400 border-amber-500/25",
  "bg-rose-500/15 text-rose-600 dark:text-rose-400 border-rose-500/25",
  "bg-cyan-500/15 text-cyan-600 dark:text-cyan-400 border-cyan-500/25",
  "bg-orange-500/15 text-orange-600 dark:text-orange-400 border-orange-500/25",
  "bg-indigo-500/15 text-indigo-600 dark:text-indigo-400 border-indigo-500/25",
];

export function hashAgentName(name: string): number {
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = ((hash << 5) - hash + name.charCodeAt(i)) | 0;
  }
  return Math.abs(hash);
}

// ── Avatar ───────────────────────────────────────────────────────────────────

export function RoleAvatar({
  role,
  iconUrl,
  agentName,
}: {
  role: string;
  iconUrl?: string | null;
  agentName?: string;
}) {
  const isUser = role === "user";
  const isAgentSender = role === "agent-sender";

  if (isUser && !isAgentSender) {
    return (
      <Avatar className="h-9 w-9 rounded-xl shadow-sm">
        <AvatarFallback className="rounded-xl bg-primary/10 border border-primary/20 text-primary">
          <User className="h-4 w-4" />
        </AvatarFallback>
      </Avatar>
    );
  }

  const colorIdx = agentName ? hashAgentName(agentName) % AGENT_COLORS.length : 0;
  return (
    <Avatar className="h-9 w-9 rounded-xl shadow-sm">
      {iconUrl && <AvatarImage src={iconUrl} alt={agentName || "agent"} className="rounded-xl object-cover" />}
      <AvatarFallback className={`rounded-xl text-sm font-semibold border ${agentName ? AGENT_COLORS[colorIdx] : "bg-muted/50 border-border text-muted-foreground"}`}>
        {agentName ? agentName[0].toUpperCase() : <Bot className="h-4 w-4" />}
      </AvatarFallback>
    </Avatar>
  );
}

// ── Multi-agent visual elements ─────────────────────────────────────────────

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

export function AgentJoinedMessage({ agentName }: { agentName: string }) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center justify-center gap-2 py-2 text-xs text-muted-foreground/50">
      <div className="h-px flex-1 bg-border/30" />
      <span>{t("chat.agent_joined", { agent: agentName })}</span>
      <div className="h-px flex-1 bg-border/30" />
    </div>
  );
}

// ── Part renderers (exported for MessageItem.tsx) ───────────────────────────

export function ToolCallPartView({ toolName, args, result, status }: {
  toolName: string;
  args: Record<string, unknown>;
  result?: unknown;
  status: { type: string };
}) {
  const { t } = useTranslation();
  const isComplete = status.type === "complete";
  const hasError = status.type === "error";
  const isDenied = status.type === "denied";
  const isRunning = status.type === "running" || status.type === "requires-action";
  const hasContent = isComplete || hasError || isDenied;

  const progress = useToolProgress(isRunning);

  const inputDisplay = args && Object.keys(args).length > 0
    ? JSON.stringify(args, null, 2)
    : null;

  const TOOL_OUTPUT_MAX_CHARS = 10_000;
  const resultRaw = result
    ? typeof result === "string" ? result : JSON.stringify(result, null, 2)
    : "";
  const [showFullOutput, setShowFullOutput] = useState(false);
  const { text: resultDisplay, truncated: resultTruncated, hiddenChars: resultHiddenChars } =
    showFullOutput
      ? { text: resultRaw, truncated: false, hiddenChars: 0 }
      : truncateOutput(resultRaw, TOOL_OUTPUT_MAX_CHARS);

  return (
    <Collapsible
      disabled={!hasContent && !inputDisplay}
      className="group overflow-hidden rounded-xl border border-border/60 bg-card/50 dark:bg-card/30 transition-all hover:border-primary/40 dark:hover:bg-card/50"
    >
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className="flex w-full items-center gap-3 px-4 py-3 text-left transition-colors"
        >
          <div
            className={`h-2.5 w-2.5 rounded-full shrink-0 ${
              hasError || isDenied
                ? "bg-destructive shadow-lg shadow-destructive/30"
                : isComplete
                  ? "bg-success shadow-lg shadow-success/30"
                  : "bg-warning animate-pulse shadow-lg shadow-warning/30"
            }`}
          />
          <span className="font-mono text-xs font-semibold tracking-tight text-foreground truncate">
            {toolName}
          </span>
          <div className="ml-auto flex items-center gap-2 shrink-0">
            {isRunning && (
              <div className="w-[60px]">
                <div className="h-0.5 w-full rounded-full bg-border/40 overflow-hidden">
                  <div
                    className="h-full rounded-full bg-warning/60 transition-all duration-200 ease-out"
                    style={{ width: `${progress * 100}%` }}
                  />
                </div>
              </div>
            )}
            <span className={`font-mono text-[10px] font-bold uppercase tracking-widest ${
              hasError || isDenied
                ? "text-destructive"
                : isComplete
                  ? "text-success"
                  : "text-muted-foreground/50"
            }`}>
              {hasError ? "ERR" : isDenied ? "DENY" : isComplete ? "OK" : "..."}
            </span>
            {(hasContent || inputDisplay) && (
              <ChevronRight
                className="h-4 w-4 text-muted-foreground/40 transition-transform duration-300 group-data-[state=open]:rotate-90"
              />
            )}
          </div>
        </button>
      </CollapsibleTrigger>

      <CollapsibleContent>
        <div className="border-t border-border/50 bg-muted/40 dark:bg-muted/20">
          {inputDisplay && (
            <div className="border-b border-border/30 p-3">
              <div className="flex items-center gap-2 mb-1.5">
                <span className="font-mono text-[10px] font-bold uppercase tracking-wider text-primary/70">
                  {t("chat.tool_input")}
                </span>
              </div>
              <pre className="max-h-[150px] overflow-auto whitespace-pre-wrap font-mono text-xs leading-relaxed text-foreground/80 dark:text-foreground/60">
                {inputDisplay}
              </pre>
            </div>
          )}
          {(isComplete || hasError || isDenied) && (
            <div className="p-3">
              <div className="flex items-center gap-2 mb-1.5">
                <span className={`font-mono text-[10px] font-bold uppercase tracking-wider ${
                  hasError || isDenied ? "text-destructive" : "text-success"
                }`}>
                  {hasError ? t("chat.tool_error") : isDenied ? t("chat.tool_denied") : t("chat.tool_result")}
                </span>
              </div>
              <pre className="max-h-[300px] overflow-auto whitespace-pre-wrap font-mono text-xs leading-relaxed text-foreground/90 dark:text-foreground/70">
                {resultDisplay}
                {resultTruncated && (
                  <button
                    type="button"
                    onClick={() => setShowFullOutput(true)}
                    className="mt-2 text-xs text-primary/70 hover:text-primary underline underline-offset-2"
                  >
                    Show {Math.round(resultHiddenChars / 1000)}K more characters…
                  </button>
                )}
              </pre>
            </div>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

export function FileDataPartView({ data }: { data: { url: string; mediaType: string } }) {
  const { url, mediaType } = data;
  const safeUrl = sanitizeUrl(url);
  if (mediaType.startsWith("image/")) {
    return (
      <a href={safeUrl} target="_blank" rel="noopener noreferrer">
        <img src={safeUrl} alt="" className="max-w-md rounded-xl border border-border" loading="lazy" />
      </a>
    );
  }
  if (mediaType.startsWith("audio/")) {
    return <audio controls src={safeUrl} className="w-full max-w-md" />;
  }
  if (mediaType.startsWith("video/")) {
    return <video controls src={safeUrl} className="max-w-md rounded-xl border border-border" />;
  }
  return (
    <a href={safeUrl} target="_blank" rel="noopener noreferrer" className="text-sm text-primary underline">
      {mediaType} file
    </a>
  );
}

export function SourceUrlDataPartView({ data }: { data: { url: string; title?: string } }) {
  return (
    <a
      href={sanitizeUrl(data.url)}
      target="_blank"
      rel="noopener noreferrer"
      className="inline-flex items-center gap-1.5 rounded-lg border border-border/50 bg-muted/30 px-3 py-1.5 text-xs text-primary hover:bg-muted/50 transition-colors"
    >
      <span className="truncate max-w-[200px]">{data.title || data.url}</span>
    </a>
  );
}

export function RichCardDataPartView({ data }: { data: Record<string, unknown> }) {
  const { cardType, ...rest } = data;
  // Render agent-turn cards as visual separators instead of generic rich cards
  if (cardType === "agent-turn" && typeof rest.agentName === "string") {
    return <AgentTurnSeparator data={{ agentName: rest.agentName as string, reason: (rest.reason as string) ?? "" }} animate={false} />;
  }
  return <RichCard part={{ type: "rich-card", cardType: cardType as "table" | "metric", data: rest }} />;
}

// ── Empty state ──────────────────────────────────────────────────────────────

function EmptyState() {
  const { t } = useTranslation();
  const currentAgent = useChatStore((s) => s.currentAgent);
  const agentIcons = useAuthStore((s) => s.agentIcons);
  const agentIconUrl = currentAgent && agentIcons[currentAgent] ? `/uploads/${agentIcons[currentAgent]}` : null;

  return (
    <div className="flex h-full flex-col items-center justify-center p-6 text-center">
      <div className="relative mb-8">
        <div className="absolute inset-0 rounded-2xl bg-primary/20 blur-2xl" />
        <div className="relative flex h-24 w-24 items-center justify-center rounded-2xl border border-border/50 bg-card shadow-xl overflow-hidden">
          <div className="absolute inset-0 rounded-2xl bg-gradient-to-br from-primary/5 to-transparent" />
          {agentIconUrl ? (
            <img src={agentIconUrl} alt={currentAgent} className="h-full w-full object-cover" />
          ) : (
            <Bot className="h-12 w-12 text-primary/70" />
          )}
        </div>
        <div className="absolute -bottom-1 -right-1 h-4 w-4 rounded-full border-2 border-card bg-success animate-pulse" />
      </div>
      <h2 className="mb-2 font-display text-lg font-bold uppercase tracking-widest text-foreground/80">
        {currentAgent || t("chat.ready")}
      </h2>
      <p className="max-w-xs font-sans text-sm leading-relaxed text-muted-foreground/60">
        {t("chat.write_message_to_start")}
      </p>
      <div className="mt-6 flex flex-wrap gap-2 justify-center max-w-md">
        {[
          { key: "chat.suggestion_news", prompt: t("chat.suggestion_news"), delay: "delay-0" },
          { key: "chat.suggestion_search", prompt: t("chat.suggestion_search"), delay: "delay-75" },
          { key: "chat.suggestion_tool", prompt: t("chat.suggestion_tool"), delay: "delay-150" },
        ].map((s) => (
          <button
            key={s.key}
            onClick={() => useChatStore.getState().sendMessage(s.prompt)}
            className={`animate-in fade-in slide-in-from-bottom-1 duration-300 ${s.delay} rounded-lg border border-border/50 bg-card/50 px-4 py-2.5 text-sm text-foreground/70 hover:bg-primary/10 hover:border-primary/30 hover:text-foreground transition-all cursor-pointer`}
          >
            {s.prompt}
          </button>
        ))}
      </div>
    </div>
  );
}

// ── Model dropdown ────────────────────────────────────────────────────────────

function ModelDropdown({ agent }: { agent: string }) {
  const modelOverride = useChatStore(s => s.agents[agent]?.modelOverride ?? null);
  const { data: allAgents } = useAgents();
  const { data: allProviders = [] } = useProviders();
  const agentInfo = allAgents?.find(a => a.name === agent);
  const providerConnection = agentInfo?.provider_connection;
  const selectedProvider = allProviders.filter(p => p.type === "text").find(p => p.name === providerConnection);
  const defaultModel = agentInfo?.model ?? "";
  const { data: models } = useProviderModels(selectedProvider?.id ?? null);

  const currentModel = modelOverride ?? defaultModel;
  const shortModel = currentModel.split("/").pop()?.split(":")[0] ?? currentModel;

  if (!models || models.length <= 1) return null;

  return (
    <Select
      value={currentModel}
      onValueChange={(val) => {
        useChatStore.getState().setModelOverride(agent, val === defaultModel ? null : val);
      }}
    >
      <SelectTrigger className="h-6 border-0 bg-transparent text-[10px] font-mono uppercase tracking-wide text-muted-foreground/40 hover:text-foreground px-1 gap-1 w-auto max-w-[130px] focus:ring-0">
        <SelectValue>{shortModel}</SelectValue>
      </SelectTrigger>
      <SelectContent className="border-border text-xs">
        {(models as string[]).map((m) => (
          <SelectItem key={m} value={m} className="font-mono text-xs">
            {m === defaultModel ? `${m} ★` : m}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

// ── @-mention autocomplete ──────────────────────────────────────────────────

export function MentionAutocomplete({ query, agents, onSelect }: {
  query: string;
  agents: string[];
  onSelect: (name: string) => void;
}) {
  const q = query.toLowerCase();
  const filtered = agents.filter(p => p.toLowerCase().startsWith(q));

  if (filtered.length === 0) return null;

  return (
    <div className="absolute bottom-full mb-1 left-0 bg-popover border border-border rounded-lg shadow-lg p-1 z-50">
      {filtered.map(name => (
        <button
          key={name}
          className="flex items-center gap-2 px-3 py-1.5 text-sm rounded-md hover:bg-muted w-full text-left"
          onMouseDown={(e) => { e.preventDefault(); onSelect(name); }}
        >
          <span className="font-semibold">@{name}</span>
        </button>
      ))}
    </div>
  );
}

// ── Composer ──────────────────────────────────────────────────────────────────

interface AttachmentEntry {
  id: string;
  name: string;
  file: File;
  content: Array<{ type: string; data: string; mimeType: string; filename?: string }>;
}

function ChatComposer() {
  const { t } = useTranslation();
  const currentAgent = useChatStore((s) => s.currentAgent);
  const agents = useAuthStore((s) => s.agents);
  const viewMode = useChatStore((s) => s.agents[s.currentAgent]?.viewMode ?? "live");
  const streamStatus = useChatStore((s) => s.agents[s.currentAgent]?.streamStatus ?? "idle");
  const liveMessages = useChatStore((s) => s.agents[s.currentAgent]?.liveMessages ?? EMPTY_LIVE_MESSAGES);
  const isStreaming = streamStatus === "submitted" || streamStatus === "streaming";
  const hasMessages = viewMode === "live" ? liveMessages.length > 0 : true;
  const [slashQuery, setSlashQuery] = useState<string | null>(null);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [resolvedMention, setResolvedMention] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<AttachmentEntry[]>([]);
  const formRef = useRef<HTMLFormElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [hasInput, setHasInput] = useState(false);

  // Focus textarea on desktop only (avoid opening mobile keyboard on page load)
  useEffect(() => {
    if (window.innerWidth >= 1024) {
      textareaRef.current?.focus();
    }
  }, []);

  // Auto-resize textarea
  const autoResize = useCallback(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${ta.scrollHeight}px`;
  }, []);

  const handleComposerInput = useCallback((e: React.FormEvent<HTMLFormElement>) => {
    const ta = e.target instanceof HTMLTextAreaElement ? e.target : null;
    if (!ta) return;
    setHasInput(!!ta.value.trim());
    autoResize();
    const val = ta.value;
    if (val.startsWith("/") && !val.includes(" ")) {
      setSlashQuery(val);
      setMentionQuery(null);
    } else {
      setSlashQuery(null);
      // Detect @mention at end of input (preceded by whitespace or SOL)
      const match = val.match(/(?:^|\s)@(\w*)$/);
      setMentionQuery(match ? match[1] : null);
    }
    // Clear resolvedMention if @AgentName was removed from textarea
    setResolvedMention((prev) => {
      if (!prev) return null;
      const mentionPattern = new RegExp(`@${prev}\\b`);
      return mentionPattern.test(val) ? prev : null;
    });
  }, [autoResize]);

  const handleMentionSelect = useCallback((name: string) => {
    setMentionQuery(null);
    setResolvedMention(name);
    const ta = textareaRef.current;
    if (!ta) return;
    const val = ta.value;
    const match = val.match(/(?:^|\s)@(\w*)$/);
    if (match) {
      const before = val.slice(0, (match.index ?? 0) + (match[0].startsWith(" ") ? 1 : 0));
      const newVal = `${before}@${name} `;
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
      setter?.call(ta, newVal);
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      ta.focus();
    }
  }, []);

  const clearResolvedMention = useCallback(() => {
    setResolvedMention(null);
    const ta = textareaRef.current;
    if (!ta) return;
    const val = ta.value;
    const cleaned = val.replace(/@\w+\s?/, "").trim();
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    setter?.call(ta, cleaned);
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    ta.focus();
  }, []);

  const handleSlashSelect = useCallback((cmd: string) => {
    setSlashQuery(null);
    const ta = textareaRef.current;
    if (ta) {
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
      setter?.call(ta, "");
      ta.dispatchEvent(new Event("input", { bubbles: true }));
    }
    const store = useChatStore.getState();
    if (cmd === "/stop")           { store.stopStream(); return; }
    if (cmd === "/new")            { store.newChat(); return; }
    if (cmd.startsWith("/think:")) { store.setThinkingLevel(parseInt(cmd.split(":")[1])); return; }
    // /reset and other commands are sent as messages — backend (engine_commands.rs) handles them
    store.sendMessage(cmd);
  }, [currentAgent]);

  const handleSlashClose = useCallback(() => {
    setSlashQuery(null);
  }, []);

  const handleFileAdd = useCallback(async (file: File) => {
    const formData = new FormData();
    formData.append("file", file);
    const resp = await fetch("/api/media/upload", {
      method: "POST",
      headers: { Authorization: `Bearer ${getToken()}` },
      body: formData,
    });
    if (!resp.ok) throw new Error(`Upload failed: ${resp.status}`);
    const result = await resp.json();
    setAttachments((prev) => [
      ...prev,
      {
        id: crypto.randomUUID(),
        name: file.name,
        file,
        content: [{ type: "file", data: result.url as string, mimeType: file.type, filename: file.name }],
      },
    ]);
  }, []);

  const handleSubmit = useCallback((e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const text = textareaRef.current?.value?.trim() ?? "";
    if (!text && attachments.length === 0) return;
    useChatStore.getState().sendMessage(text);
    setAttachments([]);
    setHasInput(false);
    if (textareaRef.current) {
      textareaRef.current.value = "";
      textareaRef.current.style.height = "auto";
    }
  }, [attachments]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      formRef.current?.requestSubmit();
    }
  }, []);

  // ── Paste and drag-drop file attachment ──────────────────────────────────

  const [dragOver, setDragOver] = useState(false);

  const handlePaste = useCallback((e: React.ClipboardEvent) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (let i = 0; i < items.length; i++) {
      if (items[i].kind === "file") {
        e.preventDefault();
        const file = items[i].getAsFile();
        if (file) handleFileAdd(file);
        return; // handle first file only
      }
    }
    // If no files, let default paste behavior (text) proceed
  }, [handleFileAdd]);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(true);
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);
  }, []);

  const handleDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);
    const files = e.dataTransfer?.files;
    if (files && files.length > 0) {
      handleFileAdd(files[0]);
    }
  }, [handleFileAdd]);

  return (
    <div className="shrink-0 w-full p-3 md:p-4 border-t border-border/50 bg-background/80 backdrop-blur-sm">
      <div className="mx-auto max-w-4xl">
        <form
          ref={formRef}
          data-composer-input
          className={cn(
            "relative flex flex-col rounded-xl border bg-card/90 shadow-lg shadow-black/8 transition-all duration-200 focus-within:border-primary/50 focus-within:shadow-primary/8 focus-within:shadow-xl",
            dragOver ? "border-primary/70 bg-primary/5" : "border-border/50"
          )}
          onPaste={handlePaste}
          onDragOver={handleDragOver}
          onDragLeave={handleDragLeave}
          onDrop={handleDrop}
          onInput={handleComposerInput}
          onSubmit={handleSubmit}
        >
          {dragOver && (
            <div className="absolute inset-0 z-20 flex items-center justify-center rounded-xl border-2 border-dashed border-primary/50 bg-primary/5 backdrop-blur-sm pointer-events-none">
              <div className="flex flex-col items-center gap-1 text-primary/70">
                <Paperclip className="h-6 w-6" />
                <span className="text-sm font-medium">Drop file to attach</span>
              </div>
            </div>
          )}
          {slashQuery !== null && (
            <SlashMenu
              query={slashQuery}
              onSelect={handleSlashSelect}
              onClose={handleSlashClose}
            />
          )}
          {mentionQuery !== null && agents.length > 1 && (
            <MentionAutocomplete
              query={mentionQuery}
              agents={agents.filter(a => a !== currentAgent)}
              onSelect={handleMentionSelect}
            />
          )}
          {attachments.length > 0 && attachments.map((att) => (
            <div key={att.id} className="flex items-center gap-2 px-3 pt-2 text-xs text-muted-foreground">
              <Paperclip className="h-3 w-3" />
              <span className="truncate max-w-[200px]">{att.name}</span>
              <button
                type="button"
                onClick={() => setAttachments((prev) => prev.filter((a) => a.id !== att.id))}
                className="rounded p-0.5 hover:bg-muted/50 text-muted-foreground/60 hover:text-muted-foreground transition-colors"
              >
                <X size={12} />
              </button>
            </div>
          ))}
          <textarea
            ref={textareaRef}
            rows={1}
            enterKeyHint="send"
            autoCorrect="off"
            autoCapitalize="sentences"
            placeholder={
              viewMode === "history"
                ? t("chat.continue_dialog")
                : t("chat.message_placeholder")
            }
            className="min-h-[44px] max-h-[120px] md:max-h-[240px] resize-none bg-transparent px-4 py-3 text-[15px] text-foreground outline-none placeholder:text-muted-foreground/35"
            onKeyDown={handleKeyDown}
          />
          {resolvedMention && (
            <div data-testid="target-agent-indicator" className="flex items-center gap-1.5 px-4 py-1 text-xs text-muted-foreground">
              <span>Targeting</span>
              <span className="font-semibold text-primary">@{resolvedMention}</span>
              <button
                type="button"
                onClick={clearResolvedMention}
                className="rounded p-0.5 hover:bg-muted/50 text-muted-foreground/60 hover:text-muted-foreground transition-colors"
              >
                <X size={12} />
              </button>
            </div>
          )}
          <div className="flex items-center justify-between px-3 pb-3">
            <div className="flex items-center gap-2">
              <input
                ref={fileInputRef}
                type="file"
                accept="image/*,audio/*,video/*,application/pdf,.txt,.md,.json,.csv"
                className="hidden"
                onChange={(e) => {
                  const file = e.target.files?.[0];
                  if (file) handleFileAdd(file);
                  e.target.value = "";
                }}
              />
              <button
                type="button"
                className="rounded p-3 md:p-2 text-muted-foreground/50 hover:text-muted-foreground hover:bg-muted/50 transition-colors"
                onClick={() => fileInputRef.current?.click()}
              >
                <Paperclip className="h-4 w-4" />
              </button>
              {agents.length > 1 && (
                <span className="font-mono text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/50 bg-muted/30 px-2 py-0.5 rounded">
                  {currentAgent}
                </span>
              )}
              <ModelDropdown agent={currentAgent} />
            </div>
            <div className="relative flex items-center gap-2">
              {hasMessages && !isStreaming && (
                <button
                  type="button"
                  title={t("chat.export_session_tooltip")}
                  className="rounded p-3 md:p-2 text-muted-foreground/40 hover:text-muted-foreground hover:bg-muted/50 transition-colors"
                  onClick={() => useChatStore.getState().exportSession()}
                >
                  <Download className="h-4 w-4" />
                </button>
              )}
              {isStreaming && (
                <Button
                  type="button"
                  size="icon"
                  onClick={() => useChatStore.getState().stopStream()}
                  className="h-11 w-11 md:h-10 md:w-10 rounded-xl border border-destructive/30 bg-destructive/15 text-destructive hover:bg-destructive/25 hover:border-destructive/50 shadow-sm animate-in fade-in zoom-in-90"
                >
                  <Square className="h-3.5 w-3.5 fill-current" />
                </Button>
              )}
              <Button
                type="submit"
                size="icon"
                disabled={!hasInput && attachments.length === 0}
                className="h-11 w-11 md:h-10 md:w-10 rounded-xl border border-primary/30 bg-primary/15 text-primary hover:bg-primary/25 hover:border-primary/50 shadow-sm disabled:opacity-30 disabled:shadow-none group/send animate-in fade-in zoom-in-90"
              >
                <Send className="h-4 w-4 transition-transform duration-200 group-hover/send:translate-x-0.5 group-hover/send:-translate-y-0.5" />
              </Button>
            </div>
          </div>
        </form>
      </div>
    </div>
  );
}

// ── Read-only footer ─────────────────────────────────────────────────────────

function ReadOnlyFooter({ activeSession }: { activeSession?: SessionRow }) {
  const { t } = useTranslation();
  const label =
    activeSession?.channel === "heartbeat" ? t("chat.heartbeat_session") :
    activeSession?.channel === "cron" ? t("chat.cron_session") :
    activeSession?.channel === "group" ? t("chat.group_chat") :
    t("chat.inter_agent_session");

  return (
    <div className="shrink-0 w-full px-3 md:px-4 py-3 border-t border-primary/20 bg-primary/5">
      <div className="mx-auto max-w-4xl text-center text-sm text-primary/60 font-medium py-1">
        {label} — {t("chat.read_only")}
      </div>
    </div>
  );
}

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

function ErrorBanner({
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
  return (
    <div className="shrink-0 px-3 md:px-4 pb-1">
      <div className="mx-auto max-w-4xl flex items-center gap-3 rounded-lg border border-destructive/30 bg-destructive/5 dark:bg-destructive/15 px-4 py-2.5 text-sm text-destructive font-medium">
        <ErrorText text={error} />
        {hasMessages && (
          <Button
            variant="ghost"
            size="xs"
            className="text-destructive hover:bg-destructive/10 shrink-0"
            onClick={onRetry}
          >
            <RotateCcw className="h-3 w-3 mr-1" />
            {t("error.retry")}
          </Button>
        )}
        <button
          onClick={onClear}
          className="shrink-0 rounded p-0.5 hover:bg-destructive/20 text-destructive/60 hover:text-destructive transition-colors"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

// ── Thread Error Boundary ────────────────────────────────────────────────────

interface ThreadErrorBoundaryProps { children: ReactNode; onRetry?: () => void }
interface ThreadErrorBoundaryState { error: string | null }

class ThreadErrorBoundary extends Component<ThreadErrorBoundaryProps, ThreadErrorBoundaryState> {
  state: ThreadErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error: error.message };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.warn("[ThreadErrorBoundary]", error.message, info.componentStack?.slice(0, 200));
  }

  render() {
    if (this.state.error) {
      return (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
          <p className="text-sm text-muted-foreground/70 font-mono">{this.state.error}</p>
          <button
            className="px-4 py-2 text-sm rounded-lg border border-border bg-card hover:bg-muted transition-colors"
            onClick={() => this.setState({ error: null })}
          >
            Retry
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

// ── Main Thread ──────────────────────────────────────────────────────────────

export function ChatThread({
  streamError,
  isReadOnly,
  activeSession,
  onClearError,
  onRetry,
}: ChatThreadProps) {
  const keyboardHeight = useVisualViewport();
  const viewMode = useChatStore((s) => s.agents[s.currentAgent]?.viewMode ?? "live");
  const currentAgent = useChatStore((s) => s.currentAgent);
  const activeSessionId = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionId ?? null);
  const streamStatus = useChatStore((s) => s.agents[s.currentAgent]?.streamStatus ?? "idle");
  const activeSessionIds = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionIds ?? []);
  const engineRunning = !isActiveStream(streamStatus) && !!activeSessionId && activeSessionIds.includes(activeSessionId);
  const { data: sessionMessagesData, isLoading: historyLoading } = useSessionMessages(
    isActiveStream(streamStatus) ? null : activeSessionId,
    engineRunning,
  );
  const renderLimit = useChatStore((s) => s.agents[s.currentAgent]?.renderLimit ?? 100);

  const liveMessages = useChatStore((s) => s.agents[s.currentAgent]?.liveMessages ?? EMPTY_LIVE_MESSAGES);
  const loadEarlierMessages = useChatStore((s) => s.loadEarlierMessages);

  // Build the resolved message array for rendering
  const historyMessages = useMemo(() => {
    if (!activeSessionId || !sessionMessagesData?.messages) return [];
    return convertHistory(sessionMessagesData.messages);
  }, [activeSessionId, sessionMessagesData]);

  // Use liveMessages while they exist (streaming + just finished).
  // Only fall back to historyMessages when liveMessages are empty (fresh page load, session switch).
  // This prevents the flash of missing agentId when switching from live→history
  // (DB may not have agent_id on intermediate tool/text message splits).
  const sourceMessages = liveMessages.length > 0 ? liveMessages : historyMessages;

  // Filter out inter-agent turn loop messages (internal routing artifacts)
  const allMessages = useMemo(() => {
    const filtered = sourceMessages.filter(m => !(m.role === "user" && m.agentId));
    return filtered.length > renderLimit ? filtered.slice(-renderLimit) : filtered;
  }, [sourceMessages, renderLimit]);

  const msgCount = sourceMessages.length;
  const hiddenCount = useMemo(() => Math.max(0, msgCount - renderLimit), [msgCount, renderLimit]);
  const hasMessages = msgCount > 0;

  const isStreaming = streamStatus === "submitted" || streamStatus === "streaming";

  // Show thinking: (1) SSE submitted, (2) server says session is active (WS-driven),
  // (3) pending target agent during turn loop (between finish of Agent A and start of Agent B)
  const pendingTarget = useChatStore((s) => s.agents[s.currentAgent]?.pendingTargetAgent ?? null);
  const showThinking = streamStatus === "submitted" || engineRunning || (isStreaming && !!pendingTarget);

  if (historyLoading) {
    return (
      <div className="flex flex-1 flex-col gap-6 p-6 max-w-4xl mx-auto">
        {[1, 2, 3].map((i) => (
          <div key={i} className="flex gap-3">
            <div className="h-9 w-9 rounded-xl bg-muted/50 animate-pulse shrink-0" />
            <div className="flex-1 space-y-2">
              <div className="h-3 w-20 rounded bg-muted/50 animate-pulse" />
              <div className="h-4 w-full rounded bg-muted/40 animate-pulse" />
              <div className="h-4 w-3/4 rounded bg-muted/30 animate-pulse" />
            </div>
          </div>
        ))}
      </div>
    );
  }

  return (
    <ThreadErrorBoundary>
    <div
      className="flex flex-1 flex-col min-h-0 relative"
      style={keyboardHeight > 0 ? { paddingBottom: keyboardHeight } : undefined}
    >
      <MessageList
        messages={allMessages}
        isStreaming={isStreaming}
        showThinking={showThinking}
        isLoadingHistory={historyLoading && liveMessages.length === 0}
        emptyState={<EmptyState />}
        hiddenCount={hiddenCount}
        onLoadEarlier={() => loadEarlierMessages(currentAgent)}
      />

      {/* Error banner */}
      {streamError && !isReadOnly && viewMode !== "history" && (
        <ErrorBanner
          error={streamError}
          hasMessages={hasMessages}
          onClear={onClearError}
          onRetry={onRetry}
        />
      )}

      {/* Input area */}
      {isReadOnly ? (
        <ReadOnlyFooter activeSession={activeSession} />
      ) : (
        <ChatComposer />
      )}
    </div>
    </ThreadErrorBoundary>
  );
}

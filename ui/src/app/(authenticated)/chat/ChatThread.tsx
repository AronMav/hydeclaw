"use client";

import React, { Component, useState, useCallback, useRef, useEffect, useMemo } from "react";
import type { ErrorInfo, ReactNode } from "react";
import { cn } from "@/lib/utils";
import { getToken } from "@/lib/api";
import { useChatStore, isActivePhase, convertHistory } from "@/stores/chat-store";
import { sanitizeUrl } from "@/lib/sanitize-url";
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

import type { SessionRow } from "@/types/api";
import { AgentTurnSeparator } from "@/components/chat/AgentTurnSeparator";

// ── Re-exports for backward compatibility ────────────────────────────────────
export { ToolCallPartView } from "@/components/chat/ToolCallPartView";
export { FileDataPartView } from "@/components/chat/FileDataPartView";
export { AgentTurnSeparator } from "@/components/chat/AgentTurnSeparator";

import { Collapsible, CollapsibleTrigger, CollapsibleContent } from "@/components/ui/collapsible";
import { BarsLoader } from "@/components/ui/loader";
import { Button } from "@/components/ui/button";
import { RichCard } from "@/components/ui/rich-card";
import { SlashMenu } from "./parts/SlashMenu";
import { MessageList, MessageSkeleton } from "./MessageList";
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
  Loader2,
  WifiOff,
  Clock,
  AlertTriangle,
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
    return <AgentTurnSeparator data={{ agentName: rest.agentName, reason: typeof rest.reason === "string" ? rest.reason : "" }} animate={false} />;
  }
  const validCardType = cardType === "table" || cardType === "metric" ? cardType : "table";
  return <RichCard part={{ type: "rich-card", cardType: validCardType, data: rest }} />;
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

// ── Draft persistence helpers ─────────────────────────────────────────────────

const DRAFT_PREFIX = "hydeclaw.draft.";

export function saveDraft(agent: string, text: string) {
  if (text) localStorage.setItem(DRAFT_PREFIX + agent, text);
  else localStorage.removeItem(DRAFT_PREFIX + agent);
}

export function loadDraft(agent: string): string {
  return localStorage.getItem(DRAFT_PREFIX + agent) ?? "";
}

export function clearDraft(agent: string) {
  localStorage.removeItem(DRAFT_PREFIX + agent);
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
  const messageSource = useChatStore((s) => s.agents[s.currentAgent]?.messageSource ?? { mode: "new-chat" as const });
  const connectionPhase = useChatStore((s) => s.agents[s.currentAgent]?.connectionPhase ?? "idle");
  const isStreaming = isActivePhase(connectionPhase);
  const hasMessages = messageSource.mode !== "new-chat";
  const [slashQuery, setSlashQuery] = useState<string | null>(null);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [resolvedMention, setResolvedMention] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<AttachmentEntry[]>([]);
  const formRef = useRef<HTMLFormElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [hasInput, setHasInput] = useState(false);
  const [uploadingCount, setUploadingCount] = useState(0);
  const isUploading = uploadingCount > 0;

  // Focus textarea on desktop only (avoid opening mobile keyboard on page load)
  useEffect(() => {
    if (window.innerWidth >= 1024) {
      textareaRef.current?.focus();
    }
  }, []);

  // Restore draft when mounting or switching agents
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    const draft = loadDraft(currentAgent);
    if (draft) {
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
      setter?.call(ta, draft);
      ta.dispatchEvent(new Event("input", { bubbles: true }));
    } else {
      // Clear textarea when switching to an agent with no draft
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
      setter?.call(ta, "");
      ta.dispatchEvent(new Event("input", { bubbles: true }));
    }
  }, [currentAgent]);

  // Auto-resize textarea — use "0px" reset instead of "auto" to prevent flicker on paste
  const autoResize = useCallback(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = "0px";
    ta.style.height = `${ta.scrollHeight}px`;
  }, []);

  const handleComposerInput = useCallback((e: React.FormEvent<HTMLFormElement>) => {
    const ta = e.target instanceof HTMLTextAreaElement ? e.target : null;
    if (!ta) return;
    setHasInput(!!ta.value.trim());
    saveDraft(currentAgent, ta.value);
    autoResize();
    const val = ta.value;
    if (val.startsWith("/") && !val.includes(" ") && !val.includes("\n") && !val.slice(1).includes("/")) {
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
    setUploadingCount(c => c + 1);
    try {
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
    } finally {
      setUploadingCount(c => c - 1);
    }
  }, []);

  const handleSubmit = useCallback((e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const text = textareaRef.current?.value?.trim() ?? "";
    if (!text && attachments.length === 0) return;
    useChatStore.getState().sendMessage(text);
    clearDraft(useChatStore.getState().currentAgent);
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
              messageSource.mode === "history"
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
                disabled={(!hasInput && attachments.length === 0) || isUploading}
                className="h-11 w-11 md:h-10 md:w-10 rounded-xl border border-primary/30 bg-primary/15 text-primary hover:bg-primary/25 hover:border-primary/50 shadow-sm disabled:opacity-30 disabled:shadow-none group/send animate-in fade-in zoom-in-90"
              >
                {isUploading
                  ? <Loader2 className="h-4 w-4 animate-spin" />
                  : <Send className="h-4 w-4 transition-transform duration-200 group-hover/send:translate-x-0.5 group-hover/send:-translate-y-0.5" />
                }
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
  const messageSource = useChatStore((s) => s.agents[s.currentAgent]?.messageSource ?? { mode: "new-chat" as const });
  const currentAgent = useChatStore((s) => s.currentAgent);
  const activeSessionId = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionId ?? null);
  const connectionPhase = useChatStore((s) => s.agents[s.currentAgent]?.connectionPhase ?? "idle");
  const activeSessionIds = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionIds ?? []);
  const engineRunning = !isActivePhase(connectionPhase) && !!activeSessionId && activeSessionIds.includes(activeSessionId);

  // Auto-resume SSE stream after page reload when engine is still processing
  const resumedRef = useRef<string | null>(null);
  useEffect(() => {
    if (engineRunning && activeSessionId && currentAgent && resumedRef.current !== activeSessionId) {
      resumedRef.current = activeSessionId;
      useChatStore.getState().resumeStream(currentAgent, activeSessionId);
    }
  }, [engineRunning, activeSessionId, currentAgent]);

  const { data: sessionMessagesData, isLoading: historyLoading } = useSessionMessages(
    isActivePhase(connectionPhase) ? null : activeSessionId,
    engineRunning,
  );
  const renderLimit = useChatStore((s) => s.agents[s.currentAgent]?.renderLimit ?? 100);

  const liveMessages = messageSource.mode === "live" ? messageSource.messages : EMPTY_LIVE_MESSAGES;
  const loadEarlierMessages = useChatStore((s) => s.loadEarlierMessages);

  // Build the resolved message array for rendering
  const historyMessages = useMemo(() => {
    if (!activeSessionId || !sessionMessagesData?.messages) return [];
    return convertHistory(sessionMessagesData.messages);
  }, [activeSessionId, sessionMessagesData]);

  // messageSource.mode is the single authority for message source selection (Fix C).
  // "live" mode: use live stream messages (may be seeded with history for F5 resume).
  // "history" mode: use React Query data.
  // "new-chat" or empty live: fall back to historyMessages (handles cases where live seed is empty).
  const sourceMessages = useMemo(() => {
    if (messageSource.mode === "live" && messageSource.messages.length > 0) {
      return messageSource.messages;
    }
    if (messageSource.mode === "history") {
      return historyMessages;
    }
    // new-chat or empty live — use historyMessages if available (e.g. WS-driven session restore)
    return historyMessages;
  }, [messageSource, historyMessages]);

  // Filter out inter-agent turn loop messages (internal routing artifacts)
  const allMessages = useMemo(() => {
    const filtered = sourceMessages.filter(m => !(m.role === "user" && m.agentId));
    return filtered.length > renderLimit ? filtered.slice(-renderLimit) : filtered;
  }, [sourceMessages, renderLimit]);

  const msgCount = sourceMessages.length;
  const hiddenCount = useMemo(() => Math.max(0, msgCount - renderLimit), [msgCount, renderLimit]);
  const hasMessages = msgCount > 0;

  const isStreaming = isActivePhase(connectionPhase);

  // Show thinking indicator only when actually waiting for a response.
  // Guard: if the last message already has assistant content, the response has started — hide thinking.
  const lastMsg = sourceMessages[sourceMessages.length - 1];
  const hasAssistantContent = lastMsg?.role === "assistant" && lastMsg.parts.length > 0;
  const showThinking = messageSource.mode === "live"
    && (connectionPhase === "submitted" || (engineRunning && !hasAssistantContent));

  // Only show loading skeleton when there is truly no data to display (Fix D).
  // If we have seeded live messages (F5 resume) or cached history, skip the skeleton.
  if (historyLoading && !sessionMessagesData && messageSource.mode !== "live") {
    return (
      <div className="flex flex-1 flex-col gap-6 p-6 max-w-4xl mx-auto">
        {[1, 2, 3].map((i) => (
          <MessageSkeleton key={i} />
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
      {streamError && !isReadOnly && messageSource.mode !== "history" && (
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

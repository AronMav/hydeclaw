"use client";

import React, { Component, useState, useCallback, useRef, useEffect, useMemo } from "react";
import type { ErrorInfo, ReactNode } from "react";
import { cn } from "@/lib/utils";
import { assertToken } from "@/lib/api";
import { useChatStore, isActivePhase, convertHistory } from "@/stores/chat-store";
import { mergeLiveOverlay } from "@/stores/chat-overlay-dedup";
import { uuid } from "@/stores/chat-types";
import { useVisualViewport } from "@/hooks/use-visual-viewport";
import type { ChatMessage } from "@/stores/chat-store";
import { useTranslation } from "@/hooks/use-translation";
import { useAuthStore } from "@/stores/auth-store";
import { useSessionMessages, useSessions } from "@/lib/queries";

import type { SessionRow } from "@/types/api";
// ── Re-exports for backward compatibility ────────────────────────────────────
export { ToolCallPartView } from "@/components/chat/ToolCallPartView";
export { FileDataPartView } from "@/components/chat/FileDataPartView";

import { Collapsible, CollapsibleTrigger, CollapsibleContent } from "@/components/ui/collapsible";
import { BarsLoader } from "@/components/ui/loader";
import { Button } from "@/components/ui/button";
import { SlashMenu } from "./parts/SlashMenu";
import { MessageList, MessageSkeleton } from "./MessageList";
import { ReconnectingIndicator } from "@/components/chat/ReconnectingIndicator";
import { EmptyState } from "./EmptyState";
import { ModelDropdown } from "./composer/ModelDropdown";
import { MentionAutocomplete } from "./composer/MentionAutocomplete";
import {
  Send,
  Square,
  ChevronRight,
  Download,
  Paperclip,
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
  }, [autoResize, currentAgent]);

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
        headers: { Authorization: `Bearer ${assertToken()}` },
        body: formData,
      });
      if (!resp.ok) throw new Error(`Upload failed: ${resp.status}`);
      const result = await resp.json();
      setAttachments((prev) => [
        ...prev,
        {
          id: uuid(),
          name: file.name,
          file,
          content: [{ type: "file", data: result.url as string, mimeType: file.type, filename: file.name }],
        },
      ]);
    } catch (err) {
      const { toast } = await import("sonner");
      toast.error(`Upload failed: ${err instanceof Error ? err.message : "unknown error"}`);
    } finally {
      setUploadingCount(c => c - 1);
    }
  }, []);

  const handleSubmit = useCallback((e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const text = textareaRef.current?.value?.trim() ?? "";
    if (!text && attachments.length === 0) return;
    useChatStore.getState().sendMessage(text, attachments);
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
  const reconnectAttempt = useChatStore((s) => s.agents[s.currentAgent]?.reconnectAttempt ?? 0);
  const maxReconnectAttempts = useChatStore((s) => s.agents[s.currentAgent]?.maxReconnectAttempts ?? 3);
  const activeSessionIds = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionIds ?? []);
  // Engine running: either WS says it's active, OR React Query sessions list says run_status=running
  const { data: sessionsData } = useSessions(currentAgent);
  const sessionRunStatus = sessionsData?.sessions?.find((s: { id: string }) => s.id === activeSessionId)?.run_status;
  
  // CRITICAL: We are "running" if we're in an active connection phase OR the DB says so.
  const engineRunning = !!activeSessionId && (
    isActivePhase(connectionPhase) || 
    activeSessionIds.includes(activeSessionId) || 
    sessionRunStatus === "running"
  );

  // Auto-resume SSE stream ONCE after page reload when engine is still processing.
  // Uses a Set to prevent re-triggering for the same session.
  const resumedSessions = useRef(new Set<string>());
  useEffect(() => {
    if (!activeSessionId || isActivePhase(connectionPhase)) return;
    if (resumedSessions.current.has(activeSessionId)) return;
    const isRunning = activeSessionIds.includes(activeSessionId) || sessionRunStatus === "running";
    if (!isRunning) return;
    resumedSessions.current.add(activeSessionId);
    useChatStore.getState().resumeStream(currentAgent, activeSessionId);
  }, [activeSessionId, activeSessionIds, sessionRunStatus, connectionPhase, currentAgent]);

  // Always fetch session messages — even during streaming.
  // During live streaming, sourceMessages prefers live data, but history data
  // is needed as fallback (e.g. F5 reload while agent is processing).
  const { data: sessionMessagesData, isLoading: historyLoading } = useSessionMessages(
    activeSessionId,
    engineRunning,
  );
  const renderLimit = useChatStore((s) => s.agents[s.currentAgent]?.renderLimit ?? 100);

  const liveMessages = messageSource.mode === "live" ? messageSource.messages : EMPTY_LIVE_MESSAGES;
  const loadEarlierMessages = useChatStore((s) => s.loadEarlierMessages);

  const selectedBranches = useChatStore((s) => s.agents[s.currentAgent]?.selectedBranches ?? {});

  // Build the resolved message array for rendering
  const historyMessages = useMemo(() => {
    if (!activeSessionId || !sessionMessagesData?.messages) return [];
    return convertHistory(sessionMessagesData.messages, false, selectedBranches);
  }, [activeSessionId, sessionMessagesData, selectedBranches]);

  // Architecture C: history + SSE overlay. See `chat-overlay-dedup.ts`
  // for the status-independent user-bubble merge (fixes the 2026-04-17
  // "sent message disappears" regression).
  const sourceMessages = useMemo(() => {
    if (messageSource.mode !== "live") {
      return historyMessages;
    }
    return mergeLiveOverlay(historyMessages, messageSource.messages);
  }, [messageSource, historyMessages]);

  // Filter out inter-agent routing messages (internal inter-agent context passed between agents).
  // These have role="user" with content starting with "[Handoff from" or "[Response from".
  // Keep the original user message (no agentId or agentId matching current agent).
  const allMessages = useMemo(() => {
    const filtered = sourceMessages.filter(m => {
      // Skip empty assistant messages (pre-content SSE placeholders) — ThinkingMessage handles this
      if (m.role === "assistant" && m.parts.length === 0) return false;
      if (m.role !== "user" || !m.agentId) return true;
      // Keep if it's from the session's primary agent (real user proxy)
      const content = m.parts[0]?.type === "text" ? (m.parts[0] as { text: string }).text : "";
      return !content.startsWith("[Handoff from") && !content.startsWith("[Response from");
    });
    return filtered.length > renderLimit ? filtered.slice(-renderLimit) : filtered;
  }, [sourceMessages, renderLimit]);

  const msgCount = sourceMessages.length;
  const hiddenCount = useMemo(() => Math.max(0, msgCount - renderLimit), [msgCount, renderLimit]);
  const hasMessages = msgCount > 0;

  const isStreaming = isActivePhase(connectionPhase);

  // Show thinking indicator when waiting for a response.
  // Cases: (1) just submitted, (2) streaming but no assistant text yet,
  //        (3) engine running server-side (e.g. during delegation — no SSE stream, but agent is working)
  const lastMsg = sourceMessages[sourceMessages.length - 1];
  const hasAssistantContent = lastMsg?.role === "assistant" && lastMsg.parts.length > 0;
  const lastMsgIsOtherAgent = lastMsg?.role === "assistant" && lastMsg.agentId && lastMsg.agentId !== currentAgent;
  const isLiveOrHistory = messageSource.mode === "live" || messageSource.mode === "history";
  const showThinking = isLiveOrHistory
    && !hasAssistantContent
    && !lastMsgIsOtherAgent
    && (connectionPhase === "submitted" || connectionPhase === "streaming" || connectionPhase === "reconnecting"
        || engineRunning || sessionRunStatus === "running");

  // Only show loading skeleton when there is truly no data to display (Fix D).
  // If we have cached history, skip the skeleton.
  // Regression 2026-04-17: previously `messageSource.mode !== "live"` skipped
  // the skeleton for live mode even when the live overlay was empty — on F5
  // during an active stream, `resumeStream` sets live:[] and history is still
  // loading, leaving the user with a BLANK chat until SSE events arrive. Now
  // we also show the skeleton when live overlay is empty AND history is still
  // loading, so the user sees a proper loading indicator instead of emptiness.
  const liveIsEmpty = messageSource.mode === "live" && messageSource.messages.length === 0;
  const showSkeleton =
    historyLoading && !sessionMessagesData &&
    (messageSource.mode !== "live" || liveIsEmpty);
  if (showSkeleton) {
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

      {/* Reconnecting indicator */}
      {connectionPhase === "reconnecting" && (
        <ReconnectingIndicator
          attempt={reconnectAttempt}
          maxAttempts={maxReconnectAttempts}
          className="my-4"
        />
      )}

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

"use client";

import React, { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { useSearchParams, useRouter } from "next/navigation";
import { useShallow } from "zustand/react/shallow";
import { useAuthStore } from "@/stores/auth-store";
import {
  useChatStore,
  isActivePhase,
  getInitialAgent,
  getLastSessionId,
} from "@/stores/chat-store";
import { useWsSubscription } from "@/hooks/use-ws-subscription";
import { useHotkey } from "@/hooks/use-hotkey";
import { ChatRuntimeProvider } from "@/providers/assistant-runtime";
import { useTranslation } from "@/hooks/use-translation";
import { relativeTime } from "@/lib/format";
import { toast } from "sonner";

import { Loader } from "@/components/ui/loader";
import { Virtuoso } from "react-virtuoso";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Sheet, SheetContent, SheetTitle, SheetTrigger } from "@/components/ui/sheet";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Avatar, AvatarImage, AvatarFallback, AvatarGroup, AvatarGroupCount } from "@/components/ui/avatar";
import {
  Plus,
  Clock,
  Search,
  Trash2,
  Pencil,
  PanelRight,
  MessageSquare,
} from "lucide-react";
import { Input } from "@/components/ui/input";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ChatThread } from "./ChatThread";
import { CanvasPanel } from "./CanvasPanel";
import { useCanvasStore } from "@/stores/canvas-store";
import { useSessions, useAgents, qk } from "@/lib/queries";
import { queryClient } from "@/lib/query-client";
import { inviteAgent } from "@/lib/api";
import type { SessionRow, AgentInfo } from "@/types/api";
import { TaskPlanPanel } from "@/components/TaskPlanPanel";

const EMPTY_SESSIONS: SessionRow[] = [];
const EMPTY_ACTIVE: string[] = [];

export default function ChatPage() {
  const { t, locale } = useTranslation();
  const searchParams = useSearchParams();
  const router = useRouter();
  const urlSessionId = searchParams.get("s");
  const { agents, refreshIfStale } = useAuthStore(
    useShallow((s) => ({ agents: s.agents, refreshIfStale: s.refreshIfStale })),
  );

  // ── Store (granular selectors to avoid re-renders during streaming) ──
  const currentAgent = useChatStore((s) => s.currentAgent);
  const { data: sessionsData, isLoading: sessionsLoading } = useSessions(currentAgent ?? "");
  const sessions = sessionsData?.sessions ?? EMPTY_SESSIONS;
  const activeSessionId = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionId ?? null);
  const activeSessionIds = useChatStore((s) => s.agents[s.currentAgent]?.activeSessionIds ?? EMPTY_ACTIVE);
  const messageSource = useChatStore((s) => s.agents[s.currentAgent]?.messageSource ?? { mode: "new-chat" as const });
  const viewingHistory = messageSource.mode === "history";
  const streamError = useChatStore((s) => s.agents[s.currentAgent]?.streamError ?? null);
  const isStreaming = isActivePhase(useChatStore((s) => s.agents[s.currentAgent]?.connectionPhase ?? "idle"));

  // Track which agents have been auto-restored (per-agent, not global boolean)
  // This preserves "new chat" state when switching A → B → A
  const restoredAgents = useRef(new Set<string>());

  // Initialize current agent on mount
  useEffect(() => {
    if (agents.length > 0 && !currentAgent) {
      const initial = getInitialAgent(agents);
      useChatStore.getState().setCurrentAgent(initial);
    }
  }, [agents, currentAgent]);

  // Sync agent state when agents list changes (e.g. after async restore)
  useEffect(() => {
    if (agents.length > 0 && currentAgent && !agents.includes(currentAgent)) {
      useChatStore.getState().setCurrentAgent(agents[0]);
    }
  }, [agents, currentAgent]);

  // Refresh agent icons if stale (>60s since last fetch)
  useEffect(() => { refreshIfStale(); }, [refreshIfStale]);

  // Detect read-only sessions (heartbeat, cron, inter-agent)
  const activeSession = sessions.find((s) => s.id === activeSessionId);
  const isReadOnly = activeSession?.channel === "heartbeat" || activeSession?.channel === "cron" || activeSession?.channel === "inter-agent";

  // Session restore on mount or agent switch.
  // Simple and robust: wait for sessions, pick the best one, select it.
  useEffect(() => {
    if (!currentAgent || sessionsLoading) return;

    // Already restored this agent — skip
    if (restoredAgents.current.has(currentAgent)) return;
    restoredAgents.current.add(currentAgent);

    const agentState = useChatStore.getState().agents[currentAgent];

    // If already streaming — don't touch
    if (isActivePhase(agentState?.connectionPhase)) return;

    // If already viewing a session — don't touch
    if (agentState?.activeSessionId) return;

    // Priority 1: URL ?s= param (deep link)
    if (urlSessionId && sessions.some((s) => s.id === urlSessionId)) {
      useChatStore.getState().selectSession(urlSessionId, currentAgent);
      return;
    }

    // Priority 2: Most recent session (sessions are sorted by recency from API)
    if (sessions.length > 0) {
      useChatStore.getState().selectSession(sessions[0].id, currentAgent);
      return;
    }

    // No sessions at all — new chat
    useChatStore.getState().newChat();
  }, [sessionsLoading, sessions, currentAgent, urlSessionId]);

  // Sync activeSessionId → URL ?s= param
  useEffect(() => {
    if (!activeSessionId) return;
    const currentUrlSession = searchParams.get("s");
    if (currentUrlSession !== activeSessionId) {
      const url = new URL(window.location.href);
      url.searchParams.set("s", activeSessionId);
      window.history.replaceState(null, "", url.pathname + url.search);
    }
  }, [activeSessionId, searchParams]);

  // Refresh session list and currently viewed session when backend finishes processing
  useWsSubscription("session_updated", useCallback(() => {
    const s = useChatStore.getState();
    queryClient.invalidateQueries({ queryKey: qk.sessions(s.currentAgent) });
    const agentState = s.agents[s.currentAgent];
    // Silently refresh history messages (no loading indicator to avoid flicker)
    if (agentState?.messageSource?.mode === "history" && agentState.activeSessionId && !isActivePhase(agentState.connectionPhase)) {
      s.refreshHistory(agentState.activeSessionId);
    }
  }, []));

  // Server-driven session status via WS agent_processing events.
  // Backend sends initial state on WS connect, then start/end events in real-time.
  // This updates activeSessionIds in Zustand — the single source of truth for "is session running?".
  useWsSubscription("agent_processing", useCallback((data: { agent: string; status: string; session_id?: string }) => {
    if (!data.session_id) return;
    const store = useChatStore.getState();
    if (data.status === "start") {
      store.markSessionActive(data.agent, data.session_id);
    } else {
      store.markSessionInactive(data.agent, data.session_id);
      // Refetch sessions to get final title, message count, run_status
      queryClient.invalidateQueries({ queryKey: qk.sessions(data.agent) });
    }
  }, []));

  // approval_requested handler moved to layout.tsx (must be visible on any page)

  const [sheetOpen, setSheetOpen] = useState(false);
  const [deletingSessionId, setDeletingSessionId] = useState<string | null>(null);
  const [sessionFilter, setSessionFilter] = useState("");
  const [renamingSessionId, setRenamingSessionId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");

  // ── Multi-select & delete state ──
  const [selectedSessions, setSelectedSessions] = useState<Set<string>>(new Set());
  const [deletingSelected, setDeletingSelected] = useState(false);
  const [deleteAllOpen, setDeleteAllOpen] = useState(false);
  const [deletingAll, setDeletingAll] = useState(false);

  // Clear selection when agent changes
  useEffect(() => {
    setSelectedSessions(new Set());
  }, [currentAgent]);

  const toggleSessionSelection = useCallback((sessionId: string) => {
    setSelectedSessions((prev) => {
      const next = new Set(prev);
      if (next.has(sessionId)) {
        next.delete(sessionId);
      } else {
        next.add(sessionId);
      }
      return next;
    });
  }, []);

  const handleDeleteSessions = useCallback(async () => {
    if (selectedSessions.size === 0) {
      setDeleteAllOpen(true);
      return;
    }
    setDeletingSelected(true);
    try {
      await Promise.all(
        Array.from(selectedSessions).map((id) =>
          useChatStore.getState().deleteSession(id),
        ),
      );
      queryClient.invalidateQueries({ queryKey: qk.sessions(currentAgent) });
      toast.success(t("chat.sessions_deleted"));
      setSelectedSessions(new Set());
    } catch {
      toast.error(t("chat.sessions_delete_error"));
    } finally {
      setDeletingSelected(false);
    }
  }, [selectedSessions, currentAgent, t]);

  const handleDeleteAll = useCallback(async () => {
    setDeletingAll(true);
    try {
      await useChatStore.getState().deleteAllSessions();
      queryClient.invalidateQueries({ queryKey: qk.sessions(currentAgent) });
      toast.success(t("chat.sessions_deleted"));
      setSelectedSessions(new Set());
    } catch {
      toast.error(t("chat.sessions_delete_error"));
    } finally {
      setDeletingAll(false);
      setDeleteAllOpen(false);
    }
  }, [currentAgent, t]);

  const handleDeleteSession = useCallback(async (e: React.MouseEvent, sessionId: string) => {
    e.stopPropagation();
    setDeletingSessionId(sessionId);
    try {
      await useChatStore.getState().deleteSession(sessionId);
      toast.success(t("chat.session_deleted"));
    } catch {
      toast.error(t("chat.session_delete_error"));
    } finally {
      setDeletingSessionId(null);
    }
  }, [t]);

  const handleNewChat = useCallback(() => {
    useChatStore.getState().newChat();
    // Focus composer input after new chat
    setTimeout(() => {
      const input = document.querySelector<HTMLTextAreaElement>('[role="textbox"], textarea[placeholder]');
      input?.focus();
    }, 100);
  }, []);

  const handleRegenerate = useCallback(() => {
    useChatStore.getState().regenerate();
  }, []);

  // Select a session; if it belongs to a different agent, select session state first then switch.
  // Mark agent as restored to prevent auto-restore from overriding the explicit selection.
  const handleSelectSession = useCallback((session: { id: string; agent_id: string }) => {
    const s = useChatStore.getState();
    if (session.agent_id === s.currentAgent) {
      s.selectSession(session.id);
    } else {
      restoredAgents.current.add(session.agent_id);
      s.selectSession(session.id, session.agent_id);
      s.setCurrentAgent(session.agent_id);
    }
    setSheetOpen(false);
  }, []);

  // Switch agent (including Group Chat virtual agent)
  const switchAgent = useCallback((target: string) => {
    restoredAgents.current.delete(target); // force session restore for new agent
    // Clear URL session param — it belongs to the previous agent
    const url = new URL(window.location.href);
    if (url.searchParams.has("s")) {
      url.searchParams.delete("s");
      window.history.replaceState(null, "", url.pathname + url.search);
    }
    useChatStore.getState().setCurrentAgent(target);
  }, []);

  const handleClearError = useCallback(() => {
    useChatStore.getState().clearError();
  }, []);

  // Global keyboard shortcuts (via useHotkey hook)

  // "/" — focus composer (from anywhere except inputs)
  useHotkey("/", (e) => {
    e.preventDefault();
    const input = document.querySelector<HTMLTextAreaElement>('[data-composer-input] textarea');
    input?.focus();
  });

  // Escape — blur active element (works even in inputs)
  useHotkey("Escape", () => {
    (document.activeElement as HTMLElement)?.blur();
  }, { allowInInput: true });

  // Ctrl/Cmd+Shift+N — new chat
  useHotkey("n", (e) => {
    e.preventDefault();
    handleNewChat();
  }, { ctrlOrMeta: true, shift: true });

  // Ctrl/Cmd+K — focus chat input (global scope)
  useHotkey("k", (e) => {
    e.preventDefault();
    const input = document.querySelector<HTMLTextAreaElement>('[data-composer-input] textarea');
    input?.focus();
  }, { ctrlOrMeta: true });

  // Agent selector component (reused in desktop header and mobile)
  const agentSelector = (
    <Select value={currentAgent} onValueChange={switchAgent} aria-label="Switch agent">
      <SelectTrigger size="sm" className="w-auto min-w-[5rem] sm:min-w-[7rem] max-w-[7rem] md:max-w-[10rem] text-xs font-semibold uppercase tracking-wide bg-card/50 border-border">
        <SelectValue />
      </SelectTrigger>
      <SelectContent className="border-border">
        {agents.map((a) => (
          <SelectItem key={a} value={a}>
            {a}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );

  // Filtered sessions
  const filteredSessions = useMemo(() =>
    sessionFilter
      ? sessions.filter((s) => {
          const q = sessionFilter.toLowerCase();
          return (
            (s.title && s.title.toLowerCase().includes(q)) ||
            (s.user_id && s.user_id.toLowerCase().includes(q)) ||
            s.channel.toLowerCase().includes(q) ||
            s.id.toLowerCase().includes(q)
          );
        })
      : sessions,
    [sessions, sessionFilter],
  );

  const agentIcons = useAuthStore((s) => s.agentIcons);

  // ── Session sidebar ──
  const sessionList = (
    <div className="flex h-full flex-col bg-sidebar">
      <TaskPlanPanel agentName={currentAgent} isStreaming={isStreaming} />
      <div className="flex items-center justify-between px-5 py-5 border-b border-border/50">
        <div className="flex flex-col gap-1">
          <span className="text-sm font-display font-semibold text-foreground">
            {t("chat.sessions")}
          </span>
          <span className="text-xs text-muted-foreground/60">
            {t("chat.sessions_count", { count: sessions.length })}
          </span>
        </div>
        <div className="flex items-center gap-1.5">
          {sessions.length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              className={`h-8 px-2 text-xs transition-colors ${
                selectedSessions.size > 0
                  ? "text-destructive bg-destructive/10 hover:bg-destructive/20"
                  : "text-muted-foreground/60 hover:text-destructive hover:bg-destructive/10"
              }`}
              onClick={handleDeleteSessions}
              disabled={deletingSelected}
              title={selectedSessions.size > 0
                ? t("chat.delete_selected")
                : t("chat.delete_all_sessions", { agent: currentAgent })}
            >
              {deletingSelected ? (
                <Loader className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Trash2 className="h-3.5 w-3.5" />
              )}
              {selectedSessions.size > 0 && (
                <span className="ml-1">{selectedSessions.size}</span>
              )}
            </Button>
          )}
          <Button
            variant="outline"
            size="sm"
            className="h-8 px-3 border-border/50 bg-muted/30 text-xs font-medium transition-all hover:bg-primary/15 hover:text-primary hover:border-primary/30"
            onClick={handleNewChat}
          >
            <Plus className="mr-1.5 h-3.5 w-3.5" /> {t("chat.new")}
          </Button>
        </div>
      </div>

      <div className="shrink-0 px-3 py-2 border-b border-border/30">
        <div className="relative">
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground/40" />
          <Input
            value={sessionFilter}
            onChange={(e) => setSessionFilter(e.target.value)}
            placeholder={t("chat.search_sessions")}
            className="h-8 pl-8 text-xs bg-muted/30 border-border/50 placeholder:text-muted-foreground/30"
          />
        </div>
      </div>
      <div className="flex-1 min-h-0 px-3 relative">
        {sessionsLoading && sessions.length === 0 ? (
          <div className="space-y-4 p-3">
            {[1, 2, 3].map((i) => (
              <div key={i} className="space-y-2">
                <Skeleton className="h-3 w-16 bg-muted/40" />
                <Skeleton className="h-4 w-full bg-muted/40" />
              </div>
            ))}
          </div>
        ) : filteredSessions.length === 0 ? (
          <div className="flex h-32 items-center justify-center rounded-lg border border-dashed border-border px-6 text-center">
            <p className="text-sm text-muted-foreground/70">
              {sessionFilter ? t("chat.no_sessions_match") : t("chat.no_sessions")}
            </p>
          </div>
        ) : (
          <>
            <Virtuoso
              data={filteredSessions}
              className="!h-full"
              itemContent={(_index, s) => {
                const isSelected = selectedSessions.has(s.id);
                const displayTitle = s.title || s.user_id || t("chat.no_title");
                return (
                  <div className="group relative pb-1.5">
                    <button
                      onClick={() => handleSelectSession(s)}
                      className={`relative flex w-full flex-col gap-1.5 rounded-lg px-4 py-3 text-left transition-all duration-300 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background pr-10 ${
                        isSelected
                          ? "bg-primary/10 ring-1 ring-primary/20"
                          : activeSessionId === s.id
                          ? "bg-accent shadow-inner"
                          : "hover:bg-accent/40"
                      }`}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <div className="flex items-center gap-1 min-w-0 flex-1">
                          <span
                            className={`shrink-0 h-5 w-5 md:h-3.5 md:w-3.5 rounded border transition-colors mr-1 flex items-center justify-center cursor-pointer ${
                              isSelected
                                ? "bg-primary border-primary"
                                : "border-border/60 bg-transparent hover:border-primary/40"
                            }`}
                            role="checkbox"
                            aria-checked={isSelected}
                            tabIndex={0}
                            onClick={(e) => { e.stopPropagation(); toggleSessionSelection(s.id); }}
                            onKeyDown={(e) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                e.stopPropagation();
                                toggleSessionSelection(s.id);
                              }
                            }}
                          >
                            {isSelected && (
                              <svg className="h-3.5 w-3.5 md:h-2.5 md:w-2.5 text-primary-foreground" viewBox="0 0 10 10" fill="none">
                                <path d="M2 5l2.5 2.5L8 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                              </svg>
                            )}
                          </span>
                          <span
                            className={`font-display text-xs font-bold uppercase tracking-tight shrink-0 ${
                              activeSessionId === s.id
                                ? "text-primary"
                                : "text-muted-foreground/80 group-hover:text-muted-foreground"
                            }`}
                          >
                            {s.channel}
                          </span>
                          {activeSessionIds.includes(s.id) ? (
                            <span className="ml-1.5 rounded px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-wider bg-success/15 text-success flex items-center gap-1 shrink-0">
                              <span className="h-2 w-2 rounded-full bg-success animate-pulse" />
                              running
                            </span>
                          ) : (s.run_status === "interrupted" || s.run_status === "timeout" || s.run_status === "failed") ? (
                            <span className="ml-1 rounded px-1 py-0.5 font-mono text-[9px] uppercase tracking-wider bg-destructive/15 text-destructive/80 shrink-0">
                              {s.run_status === "interrupted" ? t("chat.status_interrupted") : s.run_status === "timeout" ? t("chat.status_timeout") : t("chat.status_failed")}
                            </span>
                          ) : null}
                        </div>
                        {(s.participants?.length ?? 0) > 1 && (
                          <AvatarGroup className="shrink-0">
                            {s.participants!.slice(0, 3).map(name => (
                              <Avatar key={name} size="sm" className="size-5">
                                {agentIcons[name] ? (
                                  <AvatarImage src={`/uploads/${agentIcons[name]}`} alt={name} />
                                ) : null}
                                <AvatarFallback className="text-[8px] font-bold">{name[0]}</AvatarFallback>
                              </Avatar>
                            ))}
                            {s.participants!.length > 3 && (
                              <AvatarGroupCount className="size-5 text-[8px]">
                                +{s.participants!.length - 3}
                              </AvatarGroupCount>
                            )}
                          </AvatarGroup>
                        )}
                        <span className="font-mono text-xs tabular-nums text-muted-foreground/70 shrink-0">
                          {relativeTime(s.last_message_at, locale)}
                        </span>
                      </div>
                      {renamingSessionId === s.id ? (
                        <input
                          autoFocus
                          value={renameValue}
                          onChange={(e) => setRenameValue(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") {
                              e.preventDefault();
                              useChatStore.getState().renameSession(s.id, renameValue);
                              setRenamingSessionId(null);
                            } else if (e.key === "Escape") {
                              setRenamingSessionId(null);
                            }
                          }}
                          onBlur={() => {
                            if (renameValue !== (s.title || "")) {
                              useChatStore.getState().renameSession(s.id, renameValue);
                            }
                            setRenamingSessionId(null);
                          }}
                          className="w-full truncate text-sm bg-transparent border-b border-primary outline-none text-foreground placeholder:text-muted-foreground/50"
                          placeholder={t("chat.rename_session")}
                        />
                      ) : (
                        <p
                          className={`truncate text-sm transition-colors ${
                            activeSessionId === s.id
                              ? "text-foreground"
                              : "text-muted-foreground/70 group-hover:text-muted-foreground/90"
                          } ${!s.title && !s.user_id ? "italic text-muted-foreground/40" : ""}`}
                        >
                          {displayTitle}
                        </p>
                      )}
                      {activeSessionId === s.id && (
                        <div className="absolute left-0 top-1/2 -translate-y-1/2 h-8 w-[2px] rounded-full bg-primary" />
                      )}
                    </button>
                    <div className="absolute right-2 top-1/2 -translate-y-1/2 flex items-center gap-0.5 md:opacity-0 md:group-hover:opacity-100 transition-opacity duration-150">
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          onClick={(e) => {
                            e.stopPropagation();
                            setRenamingSessionId(s.id);
                            setRenameValue(s.title || "");
                          }}
                          className="text-muted-foreground/40 hover:text-foreground"
                          title={t("chat.rename_hint")}
                        >
                          <Pencil className="h-3 w-3" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          onClick={(e) => handleDeleteSession(e, s.id)}
                          disabled={deletingSessionId === s.id}
                          className="text-muted-foreground/40 hover:bg-destructive/10 hover:text-destructive"
                          title={t("chat.delete_session")}
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </Button>
                      </div>
                  </div>
                );
              }}
            />

          </>
        )}
      </div>
    </div>
  );

  // ── Main layout ──
  return (
    <ChatRuntimeProvider key={currentAgent}>
    <div className="flex h-full flex-col lg:flex-row bg-background overflow-hidden">
      {/* Desktop sidebar — visible only at lg+ */}
      <aside className="hidden w-[280px] shrink-0 flex-col border-r border-border lg:flex" aria-label="Session list">
        {sessionList}
      </aside>

      {/* Chat area */}
      <div className="flex min-w-0 flex-1 flex-col relative h-full">
        {/* Desktop header */}
        <div className="sticky top-0 z-10 hidden h-14 shrink-0 items-center gap-4 border-b border-border/50 bg-background/90 backdrop-blur-sm px-6 lg:flex">
          <div className="flex items-center gap-3">
            {agentSelector}
            <ParticipantBar sessionId={activeSessionId} currentAgent={currentAgent} />
            <ChatCanvasTabs />
          </div>
          {viewingHistory && (
            <div className="ml-auto flex items-center gap-2">
              <div className="rounded-md border border-primary/30 bg-primary/10 px-3 py-1 font-mono text-[10px] font-semibold uppercase tracking-widest text-primary/70">
                {t("chat.history")}
              </div>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => useChatStore.getState().newChat()}
                className="text-xs text-primary"
              >
                {t("chat.return_to_live")}
              </Button>
            </div>
          )}
          {streamError && (
            <div className="ml-auto flex items-center gap-1 text-destructive/60">
              <div className="h-2 w-2 rounded-full bg-destructive/60 animate-pulse" />
              <span className="text-[10px] font-mono uppercase tracking-wider">{t("chat.error")}</span>
            </div>
          )}
        </div>

        {/* Mobile/tablet floating actions — visible below lg */}
        <div className="absolute top-0 left-0 right-0 z-20 flex items-center justify-center gap-1.5 px-3 py-2 bg-background/90 backdrop-blur-sm border-b border-border/30 lg:hidden">
          {agentSelector}
          <ParticipantBar sessionId={activeSessionId} currentAgent={currentAgent} />
          <ChatCanvasTabs />
          <Sheet open={sheetOpen} onOpenChange={setSheetOpen}>
            <SheetTrigger asChild>
              <Button
                variant="outline"
                size="icon"
                className="h-11 w-11 md:h-8 md:w-8 shrink-0 border-border bg-background text-foreground shadow-md active:scale-95 transition-all"
                title={t("chat.archive")}
              >
                <Clock className="h-5 w-5 md:h-4 md:w-4" />
              </Button>
            </SheetTrigger>
            <SheetContent
              side="left"
              className="w-[85vw] border-r border-border bg-sidebar p-0"
            >
              <SheetTitle className="sr-only">{t("chat.sessions")}</SheetTitle>
              {sessionList}
            </SheetContent>
          </Sheet>
          <Button
            variant="outline"
            size="icon"
            className="h-11 w-11 md:h-8 md:w-8 shrink-0 border-primary/30 bg-primary/10 text-primary shadow-md active:scale-95 transition-all"
            onClick={handleNewChat}
            title={t("chat.new")}
          >
            <Plus className="h-5 w-5 md:h-4 md:w-4" />
          </Button>
        </div>

        {/* Messages + Composer */}
        {/* Tab content: Chat or Canvas */}
        <ChatCanvasContent
          key={currentAgent}
          currentAgent={currentAgent}
          streamError={streamError}
          isReadOnly={isReadOnly}
          activeSession={activeSession}
          onClearError={handleClearError}
          onRetry={() => { handleClearError(); handleRegenerate(); }}
        />
      </div>

    </div>

      <AlertDialog open={deleteAllOpen} onOpenChange={(o) => { if (!o) setDeleteAllOpen(false); }}>
        <AlertDialogContent className="rounded-xl border-border bg-card">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-base font-bold text-destructive">{t("chat.delete_all_confirm_title", { agent: currentAgent })}</AlertDialogTitle>
            <AlertDialogDescription className="text-sm text-muted-foreground">
              {t("chat.delete_all_confirm_description", { count: sessions.length, agent: currentAgent })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDeleteAll} disabled={deletingAll} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">
              {t("chat.delete_all")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

    </ChatRuntimeProvider>
  );
}

// ── Participant bar (multi-agent sessions) ─────────────────────────────────

export function ParticipantBar({ sessionId, currentAgent }: { sessionId: string | null; currentAgent: string }) {
  const { data: sessionsData } = useSessions(currentAgent);
  const session = sessionsData?.sessions.find((s: SessionRow) => s.id === sessionId);
  const participants = session?.participants ?? [currentAgent];
  const agentIcons = useAuthStore((s) => s.agentIcons);
  const { data: allAgents = [] } = useAgents();

  // Only show when there are multiple participants
  if (participants.length <= 1) return null;

  const available = allAgents.filter((a: AgentInfo) => !participants.includes(a.name));

  return (
    <div className="flex items-center gap-1.5">
      {participants.map((name: string) => (
        <div key={name} className="flex items-center gap-1.5 h-8 px-2.5 rounded-lg border border-border/40 bg-muted/30 text-xs font-semibold">
          <Avatar className="h-5 w-5 rounded-md">
            {agentIcons[name] && (
              <AvatarImage src={`/uploads/${agentIcons[name]}`} alt={name} className="rounded-md object-cover" />
            )}
            <AvatarFallback className="rounded-md bg-primary/20 text-[10px] font-bold text-primary">
              {name[0]}
            </AvatarFallback>
          </Avatar>
          <span className="uppercase tracking-wide">{name}</span>
        </div>
      ))}
      {sessionId && available.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="icon" className="h-8 w-8 border-border/40 bg-muted/30">
              <Plus className="h-3.5 w-3.5" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            {available.map((a: AgentInfo) => (
              <DropdownMenuItem key={a.name} onClick={async () => {
                try {
                  await inviteAgent(sessionId, a.name);
                  queryClient.invalidateQueries({ queryKey: qk.sessions(currentAgent) });
                } catch (err) {
                  console.error("Failed to invite:", err);
                }
              }}>
                {a.name}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </div>
  );
}

// ── Chat / Canvas tab switching ────────────────────────────────────────────

function ChatCanvasTabs() {
  const { t } = useTranslation();
  const panelOpen = useCanvasStore((s) => s.panelOpen);
  const setPanelOpen = useCanvasStore((s) => s.setPanelOpen);

  return (
    <Tabs value={panelOpen ? "canvas" : "chat"} onValueChange={(v) => setPanelOpen(v === "canvas")}>
      <TabsList className="h-8 bg-muted/30 border border-border/40 p-0.5">
        <TabsTrigger value="chat" className="h-full px-2 md:px-3 text-xs font-medium">
          <MessageSquare className="h-3 w-3" />
          {t("nav.chat")}
        </TabsTrigger>
        <TabsTrigger value="canvas" className="h-full px-2 md:px-3 text-xs font-medium">
          <PanelRight className="h-3 w-3" />
          {t("nav.canvas")}
        </TabsTrigger>
      </TabsList>
    </Tabs>
  );
}

function ChatCanvasContent({
  currentAgent,
  streamError,
  isReadOnly,
  activeSession,
  onClearError,
  onRetry,
}: {
  currentAgent: string;
  streamError: string | null;
  isReadOnly: boolean;
  activeSession?: import("@/types/api").SessionRow;
  onClearError: () => void;
  onRetry: () => void;
}) {
  const panelOpen = useCanvasStore((s) => s.panelOpen);

  if (panelOpen) {
    return <CanvasPanel agent={currentAgent} />;
  }

  return (
    <ChatThread
      streamError={streamError}
      isReadOnly={isReadOnly}
      activeSession={activeSession}
      onClearError={onClearError}
      onRetry={onRetry}
    />
  );
}

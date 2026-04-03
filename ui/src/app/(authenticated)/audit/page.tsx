"use client";

import { useState, useCallback, useEffect } from "react";
import { useWsSubscription } from "@/hooks/use-ws-subscription";
import { useTranslation } from "@/hooks/use-translation";
import { useAudit } from "@/lib/queries";
import { apiGet } from "@/lib/api";
import type { TranslationKey } from "@/i18n/types";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { AuditEvent } from "@/types/api";

const EVENT_TYPES: { value: string; labelKey: TranslationKey }[] = [
  { value: "_all", labelKey: "audit.event_all" },
  { value: "shell_exec", labelKey: "audit.event_shell_exec" },
  { value: "command_blocked", labelKey: "audit.event_command_blocked" },
  { value: "approval_requested", labelKey: "audit.event_approval_requested" },
  { value: "approval_resolved", labelKey: "audit.event_approval_resolved" },
  { value: "prompt_injection_detected", labelKey: "audit.event_prompt_injection" },
  { value: "compaction", labelKey: "audit.event_compaction" },
];

const EVENT_COLORS: Record<string, string> = {
  shell_exec: "bg-primary/10 text-primary",
  command_blocked: "bg-destructive/10 text-destructive",
  approval_requested: "bg-warning/10 text-warning",
  approval_resolved: "bg-success/10 text-success",
  prompt_injection_detected: "bg-destructive/10 text-destructive font-bold",
  compaction: "bg-muted text-muted-foreground",
};

const PAGE_SIZE = 50;

export default function AuditPage() {
  const { t, locale } = useTranslation();
  const [agent, setAgent] = useState("_all");
  const [eventType, setEventType] = useState("_all");
  const [search, setSearch] = useState("");
  const [offset, setOffset] = useState(0);
  const [agents, setAgents] = useState<string[]>([]);
  const [expandedId, setExpandedId] = useState<string | null>(null);

  const params: Record<string, string> = {
    limit: String(PAGE_SIZE),
    offset: String(offset),
  };
  if (agent !== "_all") params.agent = agent;
  if (eventType !== "_all") params.event_type = eventType;

  const { data: events = [], isFetching: loading, refetch } = useAudit(params);

  const hasMore = events.length >= PAGE_SIZE;

  const loadAgents = useCallback(async () => {
    try {
      const data = await apiGet<{ agents?: string[] }>("/api/status");
      setAgents(data.agents || []);
    } catch (e) { console.warn("[audit] failed to load agents:", e); }
  }, []);

  useEffect(() => { loadAgents(); }, [loadAgents]);

  useWsSubscription("audit_event", useCallback(() => {
    if (offset === 0) refetch();
  }, [offset, refetch]));

  const filtered = search
    ? events.filter((e: AuditEvent) => {
        const s = search.toLowerCase();
        return (
          e.event_type.toLowerCase().includes(s) ||
          e.agent_id.toLowerCase().includes(s) ||
          (e.actor && e.actor.toLowerCase().includes(s)) ||
          JSON.stringify(e.details).toLowerCase().includes(s)
        );
      })
    : events;

  return (
    <div className="flex h-full flex-col bg-background selection:bg-primary/20">
      {/* Top Bar */}
      <div className="z-10 flex flex-col md:flex-row md:items-center gap-4 border-b border-border/50 bg-background px-4 py-3 md:px-6 md:h-16">
        <div className="flex flex-col gap-0.5 md:mr-4">
          <h2 className="font-display text-lg font-bold tracking-tight text-foreground">{t("audit.title")}</h2>
          <p className="text-sm text-muted-foreground">{t("audit.subtitle")}</p>
        </div>

        <div className="flex flex-wrap items-center gap-2 md:gap-3 flex-1">
          <Select value={agent} onValueChange={(v) => { setAgent(v); setOffset(0); }}>
            <SelectTrigger className="h-9 min-w-[90px] sm:min-w-[120px] border-border bg-card/50 text-sm rounded-lg">
              <SelectValue placeholder={t("audit.agent_placeholder")} />
            </SelectTrigger>
            <SelectContent className="border-border rounded-lg">
              <SelectItem value="_all" className="text-sm">{t("audit.all_agents")}</SelectItem>
              {agents.map((a) => (
                <SelectItem key={a} value={a} className="text-sm">{a}</SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Select value={eventType} onValueChange={(v) => { setEventType(v); setOffset(0); }}>
            <SelectTrigger className="h-9 min-w-[100px] sm:min-w-[150px] border-border bg-card/50 text-sm rounded-lg">
              <SelectValue placeholder={t("audit.event_type_placeholder")} />
            </SelectTrigger>
            <SelectContent className="border-border rounded-lg">
              {EVENT_TYPES.map((et) => (
                <SelectItem key={et.value} value={et.value} className="text-sm">{t(et.labelKey)}</SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Input
            placeholder={t("audit.search_placeholder")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="h-9 flex-1 md:w-48 md:flex-none border-border bg-card/50 text-sm placeholder:text-muted-foreground/60 rounded-lg focus:ring-primary/20"
          />
        </div>

        <div className="flex items-center gap-4 shrink-0">
          <span className="font-mono text-xs tabular-nums text-muted-foreground hidden md:inline">
            {t("audit.events_count", { count: filtered.length })}
          </span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => refetch()}
            disabled={loading}
          >
            {t("common.refresh")}
          </Button>
        </div>
      </div>

      {/* Events List */}
      <div className="flex-1 overflow-y-auto p-4 md:p-6 scrollbar-thin">
        {loading && events.length === 0 ? (
          <div className="flex h-full items-center justify-center">
            <p className="text-sm text-muted-foreground animate-pulse">{t("common.loading")}</p>
          </div>
        ) : filtered.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-4 opacity-40">
            <div className="h-16 w-px bg-gradient-to-b from-transparent via-primary/50 to-transparent" />
            <p className="text-sm text-muted-foreground">{t("audit.no_events")}</p>
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {filtered.map((e: AuditEvent) => (
              <div
                key={e.id}
                className="group rounded-lg border border-border/50 bg-card/30 transition-colors hover:bg-card/60"
              >
                <button
                  type="button"
                  className="flex w-full items-center gap-3 px-4 py-3 text-left"
                  onClick={() => setExpandedId(expandedId === e.id ? null : e.id)}
                >
                  <span className="shrink-0 w-20 font-mono text-xs tabular-nums text-muted-foreground/70">
                    {new Date(e.created_at).toLocaleTimeString(locale === "en" ? "en-US" : "ru-RU", { hour12: false })}
                  </span>
                  <span className="shrink-0 w-20 font-mono text-xs text-muted-foreground truncate" title={e.agent_id}>
                    {e.agent_id}
                  </span>
                  <span className={`shrink-0 rounded-md px-2 py-0.5 text-xs font-medium ${EVENT_COLORS[e.event_type] || "bg-muted text-muted-foreground"}`}>
                    {e.event_type}
                  </span>
                  {e.actor && (
                    <span className="text-xs text-muted-foreground/60 truncate">
                      {t("audit.from", { actor: e.actor })}
                    </span>
                  )}
                  <span className="ml-auto text-xs text-muted-foreground/40">
                    {new Date(e.created_at).toLocaleDateString(locale === "en" ? "en-US" : "ru-RU")}
                  </span>
                  <span className="text-muted-foreground/40 transition-transform" style={{ transform: expandedId === e.id ? "rotate(90deg)" : "rotate(0)" }}>
                    ▶
                  </span>
                </button>

                {expandedId === e.id && (
                  <div className="border-t border-border/30 px-4 py-3">
                    <pre className="overflow-x-auto rounded-md bg-muted/50 p-3 font-mono text-xs text-foreground/80 whitespace-pre-wrap break-all">
                      {JSON.stringify(e.details, null, 2)}
                    </pre>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Pagination */}
      {(offset > 0 || hasMore) && (
        <div className="flex items-center justify-center gap-3 border-t border-border/50 bg-background px-4 py-3">
          <Button
            variant="outline"
            size="sm"
            disabled={offset === 0 || loading}
            onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
          >
            {t("common.back")}
          </Button>
          <span className="font-mono text-xs tabular-nums text-muted-foreground">
            {offset + 1}–{offset + filtered.length}
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={!hasMore || loading}
            onClick={() => setOffset(offset + PAGE_SIZE)}
          >
            {t("common.forward")}
          </Button>
        </div>
      )}
    </div>
  );
}

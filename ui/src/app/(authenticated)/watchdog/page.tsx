"use client";

import { useEffect, useState, useCallback } from "react";
import { apiGet, apiPost, apiPut } from "@/lib/api";
import { formatDuration } from "@/lib/format";
import { useAutoRefresh } from "@/hooks/use-auto-refresh";
import { useTranslation } from "@/hooks/use-translation";
import { Button } from "@/components/ui/button";
import { ErrorBanner } from "@/components/ui/error-banner";
import { Badge } from "@/components/ui/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Activity, Clock, Brain, Bot, User, Wrench, Zap, RefreshCw, Calendar, Database,
  CheckCircle2, XCircle, HeartPulse, AlertTriangle,
  type LucideProps,
} from "lucide-react";
import {
  Sheet, SheetContent, SheetHeader, SheetTitle, SheetDescription,
} from "@/components/ui/sheet";
import type { StatusInfo, StatsInfo } from "@/types/api";

interface ServiceStatus {
  ok: boolean;
  latency_ms: number;
  last_restart: string | null;
  error: string | null;
  flapping: boolean;
  can_restart?: boolean;
}

interface ResourceStatus {
  disk_free_gb: number;
  disk_warning: boolean;
  disk_critical: boolean;
  ram_used_percent: number;
  ram_warning: boolean;
  ram_critical: boolean;
  cpu_load_percent: number;
}

interface ContainerInfo {
  name: string;
  docker_name: string;
  status: string;
  healthy: boolean;
  group: string;
}

interface WatchdogStatus {
  last_check: string;
  uptime_secs: number;
  checks: Record<string, ServiceStatus>;
  resources: ResourceStatus | null;
  containers?: ContainerInfo[];
}

interface ChannelInfo {
  id: string;
  agent_name: string;
  channel_type: string;
  display_name: string;
  status: string;
}

interface AlertingSettings {
  alert_channel_ids: string[];
  alert_events: string[];
}

const ALL_EVENTS = ["down", "restart", "recovery", "resource"] as const;
const EVENT_LABEL_KEYS: Record<string, string> = {
  down: "watchdog.event_down",
  restart: "watchdog.event_restart",
  recovery: "watchdog.event_recovery",
  resource: "watchdog.event_resource",
};

export default function DashboardPage() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [stats, setStats] = useState<StatsInfo | null>(null);
  const [watchdog, setWatchdog] = useState<WatchdogStatus | null>(null);
  const [error, setError] = useState("");
  const [restarting, setRestarting] = useState<string | null>(null);
  const [refreshInterval, setRefreshInterval] = useState(60000);
  const [lastFetch, setLastFetch] = useState<Date | null>(null);
  const [channels, setChannels] = useState<ChannelInfo[]>([]);
  const [alertSettings, setAlertSettings] = useState<AlertingSettings>({
    alert_channel_ids: [],
    alert_events: ["down", "restart", "recovery", "resource"],
  });
  const [alertDirty, setAlertDirty] = useState(false);
  const [alertSaving, setAlertSaving] = useState(false);
  const [alertOpen, setAlertOpen] = useState(false);

  const restartContainer = async (dockerName: string) => {
    setRestarting(dockerName);
    try { await apiPost(`/api/containers/${dockerName}/restart`); } catch (e) { setError(t("watchdog.restart_failed", { error: String(e) })); }
    setTimeout(() => { setRestarting(null); fetchData(); }, 3000);
  };

  const restartService = async (name: string) => {
    setRestarting(name);
    try { await apiPost(`/api/watchdog/restart/${name}`); } catch (e) { setError(t("watchdog.restart_failed", { error: String(e) })); }
    setTimeout(() => { setRestarting(null); fetchData(); }, 5000);
  };

  const saveAlertSettings = async () => {
    setAlertSaving(true);
    try {
      await apiPut("/api/watchdog/settings", alertSettings);
      setAlertDirty(false);
    } catch (e) {
      setError(`Failed to save: ${e}`);
    }
    setAlertSaving(false);
  };

  const toggleChannel = (id: string) => {
    setAlertSettings((prev) => ({
      ...prev,
      alert_channel_ids: prev.alert_channel_ids.includes(id)
        ? prev.alert_channel_ids.filter((c) => c !== id)
        : [...prev.alert_channel_ids, id],
    }));
    setAlertDirty(true);
  };

  const toggleEvent = (event: string) => {
    setAlertSettings((prev) => ({
      ...prev,
      alert_events: prev.alert_events.includes(event)
        ? prev.alert_events.filter((e) => e !== event)
        : [...prev.alert_events, event],
    }));
    setAlertDirty(true);
  };

  const fetchData = useCallback(async (cancelled?: { current: boolean }) => {
    try {
      const [s, st, wd, chs, als] = await Promise.all([
        apiGet<StatusInfo>("/api/status"),
        apiGet<StatsInfo>("/api/stats"),
        apiGet<WatchdogStatus>("/api/watchdog/status").catch((e) => { console.warn("[watchdog] status fetch failed:", e); return null; }),
        apiGet<{ channels: ChannelInfo[] }>("/api/channels").catch((e) => { console.warn("[watchdog] channels fetch failed:", e); return { channels: [] }; }),
        apiGet<AlertingSettings>("/api/watchdog/settings").catch((e) => { console.warn("[watchdog] settings fetch failed:", e); return {
          alert_channel_ids: [] as string[],
          alert_events: ["down", "restart", "recovery", "resource"],
        }; }),
      ]);
      if (cancelled?.current) return;
      setStatus(s);
      setStats(st);
      if (wd && wd.checks) setWatchdog(wd);
      setChannels(chs.channels);
      if (!alertDirty) setAlertSettings(als);
      setLastFetch(new Date());
      setError("");
    } catch (e) {
      if (cancelled?.current) return;
      setError(`${e}`);
    }
  }, [alertDirty]);

  useEffect(() => {
    const cancelled = { current: false };
    fetchData(cancelled);
    return () => { cancelled.current = true; };
  }, [fetchData]);
  useAutoRefresh(fetchData, refreshInterval);

  const s = status;
  const st = stats;
  const wdChecks = watchdog ? Object.entries(watchdog.checks) : [];
  const allHealthy = wdChecks.length > 0 && wdChecks.every(([, v]) => v.ok);
  const res = watchdog?.resources;

  return (
    <div className="flex-1 overflow-y-auto p-4 md:p-6 lg:p-8 selection:bg-primary/20">
      {/* Header */}
      <div className="mb-8 md:mb-10 flex flex-col md:flex-row md:items-center justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h2 className="font-display text-lg font-bold tracking-tight flex items-center gap-2">
            <HeartPulse className="h-5 w-5 text-primary" />
            {t("watchdog.title")}
          </h2>
          <span className="text-sm text-muted-foreground">{t("watchdog.subtitle")}</span>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          {lastFetch && (
            <span className="text-[10px] text-muted-foreground tabular-nums">
              {lastFetch.toLocaleTimeString()}
            </span>
          )}
          <Select value={String(refreshInterval)} onValueChange={(v) => setRefreshInterval(Number(v))}>
            <SelectTrigger className="h-8 w-[80px] text-xs bg-card/50 border-border">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="5000" className="text-xs">5s</SelectItem>
              <SelectItem value="15000" className="text-xs">15s</SelectItem>
              <SelectItem value="30000" className="text-xs">30s</SelectItem>
              <SelectItem value="60000" className="text-xs">60s</SelectItem>
            </SelectContent>
          </Select>
          {res && (
            <div className="flex items-center gap-4 bg-muted/20 px-4 py-2 rounded-lg border border-border">
              <div className="flex items-center gap-1.5">
                <span className="text-[10px] text-muted-foreground">{t("watchdog.resource.disk")}</span>
                <span className={`font-mono text-sm font-bold ${res.disk_critical ? "text-destructive" : res.disk_warning ? "text-warning" : "text-foreground"}`}>{res.disk_free_gb.toFixed(0)}GB</span>
              </div>
              <div className="w-px h-4 bg-border/50" />
              <div className="flex items-center gap-1.5">
                <span className="text-[10px] text-muted-foreground">{t("watchdog.resource.ram")}</span>
                <span className={`font-mono text-sm font-bold ${res.ram_critical ? "text-destructive" : res.ram_warning ? "text-warning" : "text-foreground"}`}>{res.ram_used_percent.toFixed(0)}%</span>
              </div>
              {res.cpu_load_percent != null && (
                <>
                  <div className="w-px h-4 bg-border/50" />
                  <div className="flex items-center gap-1.5">
                    <span className="text-[10px] text-muted-foreground">{t("watchdog.resource.cpu")}</span>
                    <span className={`font-mono text-sm font-bold ${res.cpu_load_percent > 90 ? "text-destructive" : res.cpu_load_percent > 70 ? "text-warning" : "text-foreground"}`}>{res.cpu_load_percent.toFixed(0)}%</span>
                  </div>
                </>
              )}
            </div>
          )}
          <Button
            variant="outline"
            size="sm"
            onClick={() => setAlertOpen(true)}
          >
            <span className="inline-block w-1.5 h-1.5 rounded-full bg-primary/60" />
            {t("watchdog.alerting.title")}
          </Button>
        </div>
      </div>

      {error && <ErrorBanner error={error} />}

      {/* Metrics grid */}
      <div className="grid grid-cols-2 gap-3 md:gap-5 md:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
        <MetricCard
          label={t("dashboard.status")}
          value={wdChecks.length > 0 ? (allHealthy && (!watchdog?.containers || watchdog.containers.every(c => c.healthy)) ? "OK" : "ISSUES") : (s?.status?.toUpperCase() || "...")}
          dot={wdChecks.length > 0 ? (allHealthy && (!watchdog?.containers || watchdog.containers.every(c => c.healthy)) ? "success" : "error") : (s?.status === "ok" ? "success" : "error")}
          subValue={s?.version}
          icon={Activity}
        />
        <MetricCard label={t("dashboard.uptime")} value={s ? formatDuration(s.uptime_seconds) : "..."} subValue={t("dashboard.uptime_sub")} icon={Clock} />
        <MetricCard label={t("dashboard.memory")} value={s?.memory_chunks?.toLocaleString() ?? "..."} subValue={t("dashboard.memory_sub")} icon={Brain} />
        <MetricCard label={t("dashboard.agents")} value={String(s?.agents?.length ?? "0")} subValue={s?.agents?.join(", ") || t("dashboard.agents_none")} icon={Bot} />
        <MetricCard label={t("dashboard.sessions")} value={String(s?.active_sessions ?? "0")} subValue={t("dashboard.sessions_sub")} icon={User} />
        <MetricCard label={t("dashboard.tools")} value={String(s?.tools_registered ?? "0")} subValue={t("dashboard.tools_sub")} icon={Wrench} />
        <MetricCard label={t("dashboard.messages_today")} value={String(st?.messages_today ?? "0")} subValue={t("dashboard.total_messages", { value: st?.total_messages.toLocaleString() ?? "0" })} icon={Zap} />
        <MetricCard label={t("dashboard.sessions_today")} value={String(st?.sessions_today ?? "0")} subValue={t("dashboard.total_sessions", { value: st?.total_sessions.toLocaleString() ?? "0" })} icon={RefreshCw} />
        <MetricCard label={t("dashboard.scheduled_jobs")} value={String(s?.scheduled_jobs ?? "0")} subValue={t("dashboard.scheduled_sub")} icon={Calendar} />
      </div>

      {/* Alerting Settings Sheet */}
      <Sheet open={alertOpen} onOpenChange={setAlertOpen}>
        <SheetContent side="right" className="w-80 sm:max-w-sm">
          <SheetHeader>
            <SheetTitle className="text-sm">{t("watchdog.alerting.title")}</SheetTitle>
            <SheetDescription className="text-xs">
              {t("watchdog.alerting.description")}
            </SheetDescription>
          </SheetHeader>

          <div className="px-4 space-y-6 flex-1 overflow-y-auto">
            <div className="space-y-3">
              <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider">{t("watchdog.alerting.channels")}</p>
              {channels.length === 0 ? (
                <p className="text-xs text-muted-foreground italic">{t("watchdog.alerting.no_channels")}</p>
              ) : (
                <div className="flex flex-col gap-1.5">
                  {channels.map((ch) => {
                    const selected = alertSettings.alert_channel_ids.includes(ch.id);
                    return (
                      <Button
                        key={ch.id}
                        variant={selected ? "default" : "outline"}
                        size="sm"
                        role="checkbox"
                        aria-checked={selected}
                        onClick={() => toggleChannel(ch.id)}
                        className="w-full justify-start text-xs h-auto py-2"
                      >
                        <span className="font-medium">{ch.agent_name}</span>
                        <span className="opacity-70"> / {ch.channel_type}</span>
                        {ch.display_name !== ch.channel_type && (
                          <span className="opacity-50"> ({ch.display_name})</span>
                        )}
                      </Button>
                    );
                  })}
                </div>
              )}
            </div>

            <div className="space-y-3">
              <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider">{t("watchdog.alerting.events")}</p>
              <div className="flex flex-col gap-1.5">
                {ALL_EVENTS.map((event) => {
                  const selected = alertSettings.alert_events.includes(event);
                  return (
                    <Button
                      key={event}
                      variant={selected ? "default" : "outline"}
                      size="sm"
                      role="checkbox"
                      aria-checked={selected}
                      onClick={() => toggleEvent(event)}
                      className="w-full justify-start text-xs"
                    >
                      {EVENT_LABEL_KEYS[event] ? t(EVENT_LABEL_KEYS[event] as Parameters<typeof t>[0]) : event}
                    </Button>
                  );
                })}
              </div>
            </div>

          </div>

          {alertDirty && (
            <div className="px-4 pb-4">
              <Button
                onClick={saveAlertSettings}
                disabled={alertSaving}
                className="w-full"
              >
                {alertSaving ? t("common.saving") : t("common.save")}
              </Button>
            </div>
          )}
        </SheetContent>
      </Sheet>

      {/* Service health from Watchdog */}
      {wdChecks.length > 0 && (
        <div className="mt-8">
          <div className="flex items-center gap-3 mb-4">
            <HeartPulse size={16} className="text-primary/60" />
            <span className="text-sm font-semibold text-foreground">{t("watchdog.services")}</span>
            <Badge variant="outline" className="text-[10px] font-mono">
              {wdChecks.filter(([,v]) => v.ok).length}/{wdChecks.length}
            </Badge>
          </div>
          <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-3">
            {wdChecks.map(([name, svc]) => (
              <div
                key={name}
                className={`neu-card p-3 flex flex-col gap-1.5 ${
                  svc.flapping ? "border-l-[3px] border-l-warning" : !svc.ok ? "border-l-[3px] border-l-destructive" : ""
                }`}
              >
                <div className="flex items-center justify-between">
                  <span className="text-xs font-semibold text-muted-foreground">{name}</span>
                  <div className="flex items-center gap-1">
                    {svc.can_restart && (
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => restartService(name)}
                        disabled={restarting === name}
                        aria-label={t("watchdog.restart_service")}
                        className="min-h-[44px] min-w-[44px]"
                      >
                        <RefreshCw className={`h-3 w-3 text-muted-foreground ${restarting === name ? "animate-spin" : ""}`} />
                      </Button>
                    )}
                    {svc.ok ? (
                      <CheckCircle2 size={14} className="text-success" />
                    ) : svc.flapping ? (
                      <AlertTriangle size={14} className="text-warning" />
                    ) : (
                      <XCircle size={14} className="text-destructive" />
                    )}
                  </div>
                </div>
                <span className="font-mono text-xs text-foreground/60">{svc.latency_ms}ms</span>
                {svc.error && (
                  <span className="text-[10px] text-destructive truncate" title={svc.error}>{svc.error}</span>
                )}
                {svc.flapping && (
                  <Badge variant="outline" className="text-[9px] w-fit border-warning/30 text-warning">{t("watchdog.flapping")}</Badge>
                )}
                {svc.last_restart && (
                  <Badge variant="outline" className="text-[9px] w-fit border-warning/30 text-warning">{t("watchdog.restarted")}</Badge>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Agents + Infrastructure containers */}
      {watchdog?.containers && watchdog.containers.length > 0 && (() => {
        const agents = watchdog.containers.filter(c => c.group === "agent");
        const infra = watchdog.containers.filter(c => c.group !== "agent");
        return (
          <div className="mt-8 space-y-6">
            {infra.length > 0 && (
              <div>
                <div className="flex items-center gap-3 mb-3">
                  <Database size={16} className="text-primary/60" />
                  <span className="text-sm font-semibold text-foreground">{t("watchdog.infrastructure")}</span>
                  <Badge variant="outline" className="text-[10px] font-mono">
                    {infra.filter(c => c.healthy).length}/{infra.length}
                  </Badge>
                </div>
                <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-3">
                  {infra.map((c) => (
                    <div key={c.name} className={`neu-card px-3 py-2.5 flex items-center gap-2 group ${!c.healthy ? "border-l-[3px] border-l-destructive bg-destructive/5" : ""}`}>
                      <span className={`h-2 w-2 rounded-full shrink-0 ${c.healthy ? "bg-success" : "bg-destructive"}`} />
                      <div className="min-w-0 flex-1">
                        <span className="text-xs font-semibold text-foreground block">{c.name}</span>
                        <span className={`text-[10px] block ${c.healthy ? "text-muted-foreground" : "text-destructive"}`}>{c.status}</span>
                      </div>
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => restartContainer(c.docker_name)}
                        disabled={restarting === c.docker_name}
                        aria-label={t("watchdog.restart_service")}
                        className="min-h-[44px] min-w-[44px] shrink-0"
                      >
                        <RefreshCw className={`h-3 w-3 text-muted-foreground ${restarting === c.docker_name ? "animate-spin" : ""}`} />
                      </Button>
                    </div>
                  ))}
                </div>
              </div>
            )}
            {agents.length > 0 && (
              <div>
                <div className="flex items-center gap-3 mb-3">
                  <Bot size={16} className="text-primary/60" />
                  <span className="text-sm font-semibold text-foreground">{t("watchdog.agents")}</span>
                  <Badge variant="outline" className="text-[10px] font-mono">
                    {agents.filter(c => c.healthy).length}/{agents.length}
                  </Badge>
                </div>
                <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-3">
                  {agents.map((c) => (
                    <div key={c.name} className={`neu-card px-3 py-2.5 flex items-center gap-2 group ${!c.healthy ? "border-l-[3px] border-l-destructive bg-destructive/5" : ""}`}>
                      <span className={`h-2 w-2 rounded-full shrink-0 ${c.healthy ? "bg-success" : "bg-destructive"}`} />
                      <div className="min-w-0 flex-1">
                        <span className="text-xs font-semibold text-foreground block">{c.name}</span>
                        <span className={`text-[10px] block ${c.healthy ? "text-muted-foreground" : "text-destructive"}`}>{c.status}</span>
                      </div>
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => restartContainer(c.docker_name)}
                        disabled={restarting === c.docker_name}
                        aria-label={t("watchdog.restart_service")}
                        className="min-h-[44px] min-w-[44px] shrink-0"
                      >
                        <RefreshCw className={`h-3 w-3 text-muted-foreground ${restarting === c.docker_name ? "animate-spin" : ""}`} />
                      </Button>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        );
      })()}

    </div>
  );
}

function MetricCard({ label, value, subValue, dot, valueClass, icon }: {
  label: string; value: string; subValue?: string;
  dot?: "success" | "error"; valueClass?: string; icon?: React.FC<LucideProps>;
}) {
  const Icon = icon;
  return (
    <div className="group neu-card neu-hover p-5 transition-all duration-300">
      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            {Icon && <Icon className="text-primary/60 group-hover:text-primary transition-colors" size={16} />}
            <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground group-hover:text-foreground transition-colors">{label}</span>
          </div>
          {dot && <div className={`h-2 w-2 rounded-full ${dot === "success" ? "bg-success" : "bg-destructive"}`} />}
        </div>
        <div className="flex flex-col gap-1">
          <span className={`font-mono text-xl font-bold tracking-tight ${valueClass || "text-foreground"}`}>{value}</span>
          {subValue && <span className="text-xs text-muted-foreground truncate">{subValue}</span>}
        </div>
      </div>
    </div>
  );
}

"use client";

import { useEffect, useRef, useState, useCallback } from "react";
import { useWsStore } from "@/stores/ws-store";
import { useWsSubscription } from "@/hooks/use-ws-subscription";
import { useTranslation } from "@/hooks/use-translation";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { LogEntry } from "@/types/api";
import type { WsLog } from "@/types/ws";

const LEVELS = ["DEBUG", "INFO", "WARN", "ERROR"] as const;
const LEVEL_PRIORITY: Record<string, number> = { DEBUG: 0, INFO: 1, WARN: 2, ERROR: 3 };
const LEVEL_COLORS: Record<string, string> = {
  DEBUG: "text-muted-foreground/60",
  INFO: "text-primary/70",
  WARN: "text-warning/70",
  ERROR: "text-destructive font-bold",
};

export default function LogsPage() {
  const { t } = useTranslation();
  const ws = useWsStore((s) => s.ws);
  const connected = useWsStore((s) => s.connected);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [level, setLevel] = useState("INFO");
  const [search, setSearch] = useState("");
  const [autoScroll, setAutoScroll] = useState(true);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!ws || !connected) return;
    ws.send({ type: "subscribe_logs" });
    return () => { ws.send({ type: "unsubscribe_logs" }); };
  }, [ws, connected]);

  const onLog = useCallback((m: WsLog) => {
    const entry: LogEntry = {
      level: m.level || "INFO",
      message: m.message || "",
      target: m.target,
      timestamp: m.timestamp || new Date().toISOString(),
    };
    setLogs((prev) => {
      const next = [...prev, entry];
      return next.length > 5000 ? next.slice(-5000) : next;
    });
  }, []);

  useWsSubscription("log", onLog);

  useEffect(() => {
    if (autoScroll && containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [logs, autoScroll]);

  const minPriority = LEVEL_PRIORITY[level] ?? 1;
  const filtered = logs.filter((l) => {
    if ((LEVEL_PRIORITY[l.level] ?? 0) < minPriority) return false;
    if (search && !l.message.toLowerCase().includes(search.toLowerCase())) return false;
    return true;
  });

  return (
    <div className="flex h-full flex-col bg-background selection:bg-primary/20">
      {/* Top Bar */}
      <div className="z-10 flex flex-col md:flex-row md:items-center gap-4 border-b border-border/50 bg-background px-4 py-3 md:px-6 md:h-16">
        <div className="flex flex-col gap-0.5 md:mr-4">
          <h2 className="font-display text-lg font-bold tracking-tight text-foreground">{t("logs.title")}</h2>
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <span className={`h-1.5 w-1.5 rounded-full ${connected ? 'bg-success' : 'bg-destructive'}`} />
            {connected ? t("logs.connected") : t("logs.disconnected")}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2 md:gap-3 flex-1">
          <Select value={level} onValueChange={setLevel}>
            <SelectTrigger className="h-9 min-w-[80px] sm:min-w-[110px] border-border bg-card/50 font-mono text-sm rounded-lg">
              <SelectValue />
            </SelectTrigger>
            <SelectContent className="border-border rounded-lg">
              {LEVELS.map((l) => (
                <SelectItem key={l} value={l} className="font-mono text-sm">{l}</SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Input
            placeholder={t("logs.search_placeholder")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="h-9 flex-1 md:w-48 md:flex-none border-border bg-card/50 font-mono text-sm placeholder:text-muted-foreground/60 rounded-lg focus:ring-primary/20"
          />

          <div className="hidden sm:flex items-center gap-3 px-3 py-1.5 rounded-lg bg-card/50 border border-border">
            <span className="text-xs text-muted-foreground">{t("logs.autoscroll")}</span>
            <Switch checked={autoScroll} onCheckedChange={setAutoScroll} className="data-[state=checked]:bg-primary" />
          </div>
        </div>

        <div className="flex items-center justify-between md:justify-end gap-4 w-full md:w-auto mt-2 md:mt-0">
          <div className="flex sm:hidden items-center gap-2">
            <Switch checked={autoScroll} onCheckedChange={setAutoScroll} className="scale-75 data-[state=checked]:bg-primary" />
            <span className="text-xs text-muted-foreground">{t("logs.autoscroll_short")}</span>
          </div>
          
          <div className="flex items-center gap-4">
            <div className="font-mono text-xs tabular-nums text-muted-foreground">
              {t("logs.entries_count", { count: filtered.length })}
            </div>
            <Button
              variant="outline"
              size="sm"
              className="h-9 text-xs"
              onClick={() => {
                const text = filtered.map((l: { timestamp: string; level: string; target?: string; message: string }) =>
                  `${l.timestamp} [${l.level}]${l.target ? ` [${l.target}]` : ''} ${l.message}`
                ).join('\n');
                const blob = new Blob([text], { type: 'text/plain' });
                const url = URL.createObjectURL(blob);
                const a = document.createElement('a');
                a.href = url;
                a.download = `hydeclaw-logs-${new Date().toISOString().slice(0,10)}.txt`;
                a.click();
                URL.revokeObjectURL(url);
              }}
              disabled={filtered.length === 0}
            >
              {t("logs.download")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-9 text-xs text-muted-foreground hover:text-destructive hover:border-destructive/50 hover:bg-destructive/10"
              onClick={() => setLogs([])}
            >
              {t("logs.clear")}
            </Button>
          </div>
        </div>
      </div>

      {/* Logs Container */}
      <div 
        ref={containerRef} 
        className="flex-1 overflow-y-auto p-4 md:p-6 font-mono text-sm leading-relaxed scrollbar-thin"
      >
        {filtered.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-4 opacity-40">
            <div className="h-16 w-px bg-gradient-to-b from-transparent via-primary/50 to-transparent" />
            <p className="text-sm text-muted-foreground">{t("logs.waiting")}</p>
          </div>
        ) : (
          <div className="flex flex-col gap-1">
            {filtered.map((l, i) => (
              <div 
                key={i} 
                className="group flex flex-col md:flex-row gap-1 md:gap-4 py-1.5 px-3 rounded-md transition-colors hover:bg-muted/30" 
                style={{ contentVisibility: "auto", containIntrinsicSize: "auto 24px" }}
              >
                <div className="flex items-center gap-3 shrink-0">
                  <span className="w-16 text-muted-foreground/60 tabular-nums group-hover:text-muted-foreground/70 transition-colors">
                    {new Date(l.timestamp).toLocaleTimeString("ru-RU", { hour12: false })}
                  </span>
                  <span className={`w-12 font-bold uppercase tracking-tighter ${LEVEL_COLORS[l.level] || ""}`}>
                    {l.level}
                  </span>
                  {l.target && (
                    <span className="w-24 md:w-32 truncate text-primary/60 font-bold hidden sm:inline-block" title={l.target}>
                      [{l.target}]
                    </span>
                  )}
                </div>
                {l.target && (
                  <span className="text-primary/60 font-bold sm:hidden" title={l.target}>
                    [{l.target}]
                  </span>
                )}
                <span className="min-w-0 break-all text-foreground/80 group-hover:text-foreground transition-colors ml-4 md:ml-0">
                  {l.message}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

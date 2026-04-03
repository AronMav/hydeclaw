"use client";

import { useState, memo } from "react";
import { useUsage, useDailyUsage } from "@/lib/queries";
import { useTranslation } from "@/hooks/use-translation";
import { ErrorBanner } from "@/components/ui/error-banner";
import type { TranslationKey } from "@/i18n/types";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { BarChart3, Cpu, ArrowUpRight, ArrowDownRight, Zap, DollarSign, Calendar } from "lucide-react";
import type { UsageResponse, UsageSummary, DailyUsageResponse } from "@/types/api";

const PERIOD_OPTIONS: { value: string; labelKey: TranslationKey }[] = [
  { value: "1", labelKey: "usage.period_24h" },
  { value: "7", labelKey: "usage.period_7d" },
  { value: "30", labelKey: "usage.period_30d" },
  { value: "90", labelKey: "usage.period_90d" },
];

/** Chart colors for distinct metrics — intentionally different per data series */
const METRIC_COLORS = {
  messages: "text-blue-500",
  tokens: "text-emerald-500",
  cost: "text-amber-500",
  sessions: "text-purple-500",
} as const;

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

export default function UsagePage() {
  const { t } = useTranslation();
  const [days, setDays] = useState("30");

  const daysNum = Number(days);
  const { data, error: usageError, isLoading: usageLoading } = useUsage(daysNum);
  const { data: daily, error: dailyError, isLoading: dailyLoading } = useDailyUsage(daysNum);
  const isLoading = usageLoading || dailyLoading;

  const error = usageError || dailyError ? `${usageError ?? dailyError}` : "";

  const usage = data?.usage ?? [];

  // Aggregate totals
  const totalInput = usage.reduce((s, u) => s + u.total_input, 0);
  const totalOutput = usage.reduce((s, u) => s + u.total_output, 0);
  const totalCalls = usage.reduce((s, u) => s + u.call_count, 0);
  const totalTokens = totalInput + totalOutput;
  const totalCost = usage.reduce((s, u) => s + (u.estimated_cost ?? 0), 0);

  // Group by agent
  const byAgent = new Map<string, UsageSummary[]>();
  for (const u of usage) {
    const arr = byAgent.get(u.agent_id) || [];
    arr.push(u);
    byAgent.set(u.agent_id, arr);
  }

  // Find max for bar chart scaling
  const maxTotal = Math.max(1, ...usage.map((u) => u.total_input + u.total_output));

  if (isLoading) {
    return (
      <div className="flex-1 overflow-y-auto p-4 md:p-6 lg:p-8">
        <div className="space-y-6">
          {[1, 2, 3].map((i) => (
            <div key={i} className="h-32 rounded-xl border border-border bg-muted/20 animate-pulse" />
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto p-4 md:p-6 lg:p-8 selection:bg-primary/20">
      {/* Header */}
      <div className="mb-8 md:mb-10 flex flex-col md:flex-row md:items-center justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h2 className="font-display text-lg font-bold tracking-tight">{t("usage.title")}</h2>
          <span className="text-sm text-muted-foreground">
            {t("usage.subtitle")}
          </span>
        </div>
        <Select value={days} onValueChange={setDays}>
          <SelectTrigger className="w-full sm:w-44 h-9 bg-card/50 border-border text-sm">
            <SelectValue />
          </SelectTrigger>
          <SelectContent className="border-border">
            {PERIOD_OPTIONS.map((o) => (
              <SelectItem key={o.value} value={o.value}>{t(o.labelKey)}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {error && <ErrorBanner error={error} />}

      {/* Total tokens hero card */}
      {totalTokens > 0 && (
        <div className="mb-5 rounded-xl border border-amber-500/20 bg-gradient-to-r from-amber-500/5 via-card/80 to-amber-500/5 p-5 flex items-center justify-between gap-4 overflow-hidden relative">
          <div className="absolute -right-6 -top-6 opacity-[0.04]">
            <Zap className="h-32 w-32 text-amber-500" />
          </div>
          <div className="relative">
            <div className="flex items-center gap-2 mb-1">
              <Zap className="h-4 w-4 text-amber-500" />
              <span className="text-xs font-medium text-muted-foreground uppercase tracking-wide">{t("usage.total_tokens_summary")}</span>
            </div>
            <div className="text-4xl font-display font-bold tracking-tight text-amber-500">
              {formatTokens(totalTokens)}
            </div>
            <div className="text-xs text-muted-foreground/60 mt-1">
              {t("usage.period_days", { days: data?.days ?? 0 })} &middot; {totalCalls.toLocaleString()} {t("usage.calls_short")}
            </div>
          </div>
          <div className="relative flex flex-col items-end gap-1 shrink-0">
            <div className="text-right">
              <div className="text-xs text-muted-foreground/60">{t("usage.input_short")}</div>
              <div className="text-sm font-mono font-bold text-blue-500">{formatTokens(totalInput)}</div>
            </div>
            <div className="text-right">
              <div className="text-xs text-muted-foreground/60">{t("usage.output_short")}</div>
              <div className="text-sm font-mono font-bold text-emerald-500">{formatTokens(totalOutput)}</div>
            </div>
            {totalCost > 0 && (
              <div className="text-right">
                <div className="text-xs text-muted-foreground/60">{t("usage.estimated_cost")}</div>
                <div className="text-sm font-mono font-bold text-purple-500">${totalCost.toFixed(4)}</div>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Summary Cards */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3 mb-8">
        <SummaryCard
          icon={Zap}
          label={t("usage.total_tokens")}
          value={formatTokens(totalTokens)}
          sub={t("usage.period_days", { days: data?.days ?? 0 })}
          accent={METRIC_COLORS.cost}
          borderAccent="border-amber-500/30"
          gradientFrom="from-amber-500/5"
        />
        <SummaryCard
          icon={ArrowUpRight}
          label={t("usage.input_tokens")}
          value={formatTokens(totalInput)}
          sub={t("usage.pct_of_total", { pct: ((totalInput / Math.max(totalTokens, 1)) * 100).toFixed(0) })}
          accent={METRIC_COLORS.messages}
          borderAccent="border-blue-500/30"
          gradientFrom="from-blue-500/5"
        />
        <SummaryCard
          icon={ArrowDownRight}
          label={t("usage.output_tokens")}
          value={formatTokens(totalOutput)}
          sub={t("usage.pct_of_total", { pct: ((totalOutput / Math.max(totalTokens, 1)) * 100).toFixed(0) })}
          accent={METRIC_COLORS.tokens}
          borderAccent="border-emerald-500/30"
          gradientFrom="from-emerald-500/5"
        />
        <SummaryCard
          icon={DollarSign}
          label={t("usage.estimated_cost")}
          value={totalCost > 0 ? `$${totalCost.toFixed(4)}` : "$0"}
          sub={t("usage.api_calls", { count: totalCalls.toLocaleString() })}
          accent={METRIC_COLORS.sessions}
          borderAccent="border-purple-500/30"
          gradientFrom="from-purple-500/5"
        />
      </div>

      {/* Daily Chart */}
      {daily && daily.daily.length > 0 && <DailyChart data={daily.daily} />}

      {/* Per-Agent Breakdown */}
      {usage.length === 0 ? (
        <div className="flex flex-col items-center justify-center py-20 text-muted-foreground">
          <BarChart3 className="h-12 w-12 mb-3 opacity-30" />
          <p className="text-sm font-medium">{t("usage.no_data")}</p>
          <p className="text-xs mt-1 opacity-60">{t("usage.tracking_hint")}</p>
        </div>
      ) : (
        <div className="space-y-6">
          {Array.from(byAgent.entries()).map(([agent, rows]) => {
            const agentInput = rows.reduce((s, r) => s + r.total_input, 0);
            const agentOutput = rows.reduce((s, r) => s + r.total_output, 0);
            const agentCalls = rows.reduce((s, r) => s + r.call_count, 0);

            return (
              <div key={agent} className="rounded-xl border border-border bg-card/80 overflow-hidden">
                {/* Agent Header */}
                <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 px-4 sm:px-5 py-4 border-b border-border/50 bg-muted/20">
                  <div className="flex items-center gap-3">
                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-primary/10">
                      <Cpu className="h-4 w-4 text-primary" />
                    </div>
                    <div className="min-w-0">
                      <h3 className="text-sm font-bold tracking-tight truncate">{agent}</h3>
                      <span className="text-xs text-muted-foreground">
                        {t("usage.calls", { count: agentCalls.toLocaleString() })}
                      </span>
                    </div>
                  </div>
                  <div className="flex gap-4 sm:gap-6 text-right ml-11 sm:ml-0">
                    <div>
                      <div className="text-xs text-muted-foreground">{t("usage.input_short")}</div>
                      <div className="text-sm font-mono font-bold text-blue-500">{formatTokens(agentInput)}</div>
                    </div>
                    <div>
                      <div className="text-xs text-muted-foreground">{t("usage.output_short")}</div>
                      <div className="text-sm font-mono font-bold text-emerald-500">{formatTokens(agentOutput)}</div>
                    </div>
                    <div>
                      <div className="text-xs text-muted-foreground">{t("usage.total_short")}</div>
                      <div className="text-sm font-mono font-bold">{formatTokens(agentInput + agentOutput)}</div>
                    </div>
                  </div>
                </div>

                {/* Provider Rows — alternating background */}
                <div className="divide-y divide-border/30">
                  {rows.map((row, rowIdx) => {
                    const rowTotal = row.total_input + row.total_output;
                    const pct = (rowTotal / maxTotal) * 100;

                    return (
                      <div
                        key={`${row.agent_id}-${row.provider}-${row.model}`}
                        className={`relative px-4 sm:px-5 py-3 group hover:bg-muted/20 transition-colors ${
                          rowIdx % 2 === 1 ? "bg-muted/[0.04]" : ""
                        }`}
                      >
                        {/* Background bar */}
                        <div
                          className="absolute inset-y-0 left-0 bg-primary/[0.04] transition-all duration-500"
                          style={{ width: `${pct}%` }}
                        />
                        <div className="relative flex flex-col sm:flex-row sm:items-center justify-between gap-1.5 sm:gap-3">
                          <div className="flex items-center gap-3 flex-wrap">
                            <span className="inline-flex h-6 items-center rounded-md bg-muted/60 px-2 text-xs font-mono font-medium text-muted-foreground">
                              {row.provider}
                            </span>
                            {row.model && (
                              <span className="inline-flex h-6 items-center rounded-md bg-primary/10 px-2 text-xs font-mono font-medium text-primary/80">
                                {row.model}
                              </span>
                            )}
                            <span className="text-xs text-muted-foreground/60">
                              {t("usage.calls", { count: row.call_count.toLocaleString() })}
                            </span>
                          </div>
                          <div className="flex gap-3 sm:gap-5 text-right ml-0 sm:ml-auto">
                            <span className="text-xs font-mono tabular-nums text-blue-500/80">
                              {formatTokens(row.total_input)} {t("usage.input_abbr")}
                            </span>
                            <span className="text-xs font-mono tabular-nums text-emerald-500/80">
                              {formatTokens(row.total_output)} {t("usage.output_abbr")}
                            </span>
                            <span className="text-xs font-mono font-bold tabular-nums">
                              {formatTokens(rowTotal)}
                            </span>
                            {row.estimated_cost != null && (
                              <span className="text-xs font-mono tabular-nums text-purple-500/80">
                                ${row.estimated_cost.toFixed(4)}
                              </span>
                            )}
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

const DailyChart = memo(function DailyChart({ data }: { data: DailyUsageResponse["daily"] }) {
  const { t } = useTranslation();
  // Aggregate by date
  const byDate = new Map<string, { input: number; output: number; calls: number }>();
  for (const d of data) {
    const existing = byDate.get(d.date) || { input: 0, output: 0, calls: 0 };
    existing.input += d.input_tokens;
    existing.output += d.output_tokens;
    existing.calls += d.call_count;
    byDate.set(d.date, existing);
  }

  const entries = Array.from(byDate.entries()).sort(([a], [b]) => a.localeCompare(b));
  const maxTokens = Math.max(1, ...entries.map(([, v]) => v.input + v.output));

  return (
    <div className="mb-8 rounded-xl border border-border bg-card/80 overflow-hidden">
      <div className="flex items-center gap-3 px-4 sm:px-5 py-4 border-b border-border/50 bg-muted/20">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-primary/10">
          <Calendar className="h-4 w-4 text-primary" />
        </div>
        <div>
          <h3 className="text-sm font-bold tracking-tight">{t("usage.tokens_by_day")}</h3>
          <span className="text-xs text-muted-foreground">
            <span className="inline-block w-2 h-2 rounded-sm bg-blue-500 mr-1" />{t("usage.input_legend")}
            <span className="inline-block w-2 h-2 rounded-sm bg-emerald-500 ml-3 mr-1" />{t("usage.output_legend")}
          </span>
        </div>
      </div>
      <div className="px-4 sm:px-5 py-4">
        <div className="flex items-end gap-[2px] sm:gap-1" style={{ height: 160 }}>
          {entries.map(([date, val], idx) => {
            const total = val.input + val.output;
            const pct = (total / maxTokens) * 100;
            const inputPct = total > 0 ? (val.input / total) * 100 : 50;
            const shortDate = date.slice(5); // MM-DD
            const labelStep = Math.max(1, Math.ceil(entries.length / 10));
            const showLabel = entries.length <= 14 || idx % labelStep === 0;

            return (
              <div
                key={date}
                className="group relative flex-1 min-w-0 flex flex-col justify-end h-full"
              >
                {/* Tooltip */}
                <div className="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 hidden group-hover:block z-10">
                  <div className="rounded-lg border border-border bg-popover px-3 py-2 text-xs shadow-lg whitespace-nowrap">
                    <div className="font-bold mb-1">{date}</div>
                    <div className="text-blue-500">{formatTokens(val.input)} {t("usage.input_abbr")}</div>
                    <div className="text-emerald-500">{formatTokens(val.output)} {t("usage.output_abbr")}</div>
                    <div className="text-muted-foreground">{t("usage.calls", { count: val.calls })}</div>
                  </div>
                </div>
                {/* Bar */}
                <div
                  className="w-full rounded-t-sm overflow-hidden transition-all duration-300 group-hover:opacity-80 group-hover:ring-1 group-hover:ring-primary/20"
                  style={{ height: `${Math.max(pct, 2)}%` }}
                >
                  <div className="h-full flex flex-col">
                    <div className="bg-blue-500" style={{ flex: inputPct }} />
                    <div className="bg-emerald-500" style={{ flex: 100 - inputPct }} />
                  </div>
                </div>
                {showLabel ? (
                  <div className="text-[9px] text-muted-foreground/60 text-center mt-1 truncate">
                    {shortDate}
                  </div>
                ) : (
                  <div className="h-3" />
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
});

const SummaryCard = memo(function SummaryCard({
  icon: Icon,
  label,
  value,
  sub,
  accent,
  borderAccent,
  gradientFrom,
}: {
  icon: React.FC<{ className?: string }>;
  label: string;
  value: string;
  sub: string;
  accent: string;
  borderAccent?: string;
  gradientFrom?: string;
}) {
  return (
    <div className={`group relative rounded-xl border bg-gradient-to-br ${gradientFrom ?? ""} to-card/80 p-4 transition-all hover:shadow-sm overflow-hidden ${borderAccent ? `${borderAccent} hover:border-opacity-60` : "border-border hover:border-primary/20"}`}>
      <div className="absolute -right-3 -top-3 opacity-[0.04] group-hover:opacity-[0.08] transition-opacity">
        <Icon className="h-20 w-20" />
      </div>
      <div className="relative">
        <div className="flex items-center gap-2 mb-2">
          <Icon className={`h-4 w-4 ${accent}`} />
          <span className="text-xs font-medium text-muted-foreground uppercase tracking-wide">{label}</span>
        </div>
        <div className="text-2xl font-display font-bold tracking-tight">{value}</div>
        <div className="text-xs text-muted-foreground/60 mt-1">{sub}</div>
      </div>
    </div>
  );
});

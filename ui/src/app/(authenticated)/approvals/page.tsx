"use client";

import { useState } from "react";
import { useApprovals, useResolveApproval } from "@/lib/queries";
import { useTranslation } from "@/hooks/use-translation";
import { relativeTime } from "@/lib/format";
import { ErrorBanner } from "@/components/ui/error-banner";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { EmptyState } from "@/components/ui/empty-state";
import { ShieldCheck, Check, X, RefreshCw } from "lucide-react";

export default function ApprovalsPage() {
  const { t, locale } = useTranslation();
  const { data: approvals = [], isLoading, error, refetch } = useApprovals();
  const resolveApproval = useResolveApproval();
  const [actionError, setActionError] = useState("");

  const pending = approvals.filter((a) => a.status === "pending");

  const handleResolve = async (id: string, status: "approved" | "rejected") => {
    setActionError("");
    try {
      await resolveApproval.mutateAsync({ id, status });
    } catch (e) {
      setActionError(`${e}`);
    }
  };

  const errorMessage = error ? `${error}` : actionError;

  return (
    <div className="flex-1 overflow-y-auto p-4 md:p-6 lg:p-8 selection:bg-primary/20">
        <div className="mb-8 flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div>
            <h2 className="font-display text-lg font-bold tracking-tight text-foreground">
              {t("approvals.title")}
            </h2>
            <p className="text-sm text-muted-foreground mt-1">
              {t("approvals.subtitle")}
            </p>
          </div>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" onClick={() => refetch()} disabled={isLoading}>
              <RefreshCw className={`mr-2 h-4 w-4 ${isLoading ? "animate-spin" : ""}`} />
              {t("common.refresh")}
            </Button>
          </div>
        </div>

        {errorMessage && <ErrorBanner error={errorMessage} />}

        {pending.length === 0 ? (
          <EmptyState icon={ShieldCheck} text={t("approvals.no_pending")} />
        ) : (
          <div className="grid gap-4 md:gap-6">
            {pending.map((a) => (
              <div
                key={a.id}
                className="group relative flex flex-col gap-4 neu-card p-5 transition-all duration-300 hover:shadow-lg"
              >
                <div className="flex flex-col gap-3 min-w-0">
                  <div className="flex items-center gap-3 flex-wrap">
                    <h3 className="font-mono text-base font-bold text-foreground truncate">
                      {a.tool}
                    </h3>
                    <Badge
                      variant="outline"
                      className="text-xs border-primary/40 text-primary bg-primary/5"
                    >
                      {a.agent_id}
                    </Badge>
                    <Badge
                      variant="secondary"
                      className="text-xs bg-warning/20 text-warning border-warning/30"
                    >
                      {t("approvals.status_pending")}
                    </Badge>
                    <span className="ml-auto text-xs text-muted-foreground/60 font-mono tabular-nums">
                      {relativeTime(a.created_at, locale)}
                    </span>
                  </div>

                  {Object.keys(a.arguments).length > 0 && (
                    <div className="rounded-lg neu-inset p-3">
                      <pre className="font-mono text-xs leading-relaxed text-foreground/80 line-clamp-4 whitespace-pre-wrap break-words">
                        {JSON.stringify(a.arguments, null, 2)}
                      </pre>
                    </div>
                  )}
                </div>

                <div className="grid grid-cols-2 md:flex md:items-center md:justify-end gap-2 border-t border-border/50 pt-3">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleResolve(a.id, "approved")}
                    disabled={resolveApproval.isPending}
                    className="text-xs font-medium border-success/50 text-success hover:bg-success/10"
                  >
                    <Check className="h-3 w-3 mr-2" />
                    {t("approvals.approve")}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleResolve(a.id, "rejected")}
                    disabled={resolveApproval.isPending}
                    className="text-xs font-medium border-destructive/50 text-destructive hover:bg-destructive/10"
                  >
                    <X className="h-3 w-3 mr-2" />
                    {t("approvals.reject")}
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}
    </div>
  );
}

"use client";

import { useState } from "react";
import { useBackups, useCreateBackup } from "@/lib/queries";
import { getToken } from "@/lib/api";
import { formatDate, formatBytes } from "@/lib/format";
import { useTranslation } from "@/hooks/use-translation";
import { ErrorBanner } from "@/components/ui/error-banner";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Archive, Download, RefreshCw, Plus, RotateCcw, Trash2 } from "lucide-react";
import { apiDelete } from "@/lib/api";
import { toast } from "sonner";

export default function BackupsPage() {
  const { t, locale } = useTranslation();
  const { data: backups = [], isLoading, error, refetch } = useBackups();
  const createBackup = useCreateBackup();

  const [restoreTarget, setRestoreTarget] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [actionError, setActionError] = useState("");

  const handleDownload = (filename: string) => {
    const token = getToken();
    const url = `/api/backup/${encodeURIComponent(filename)}`;
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    // For auth, fetch as blob and create object URL
    fetch(url, {
      headers: { Authorization: `Bearer ${token}` },
    })
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        return r.blob();
      })
      .then((blob) => {
        const objectUrl = URL.createObjectURL(blob);
        a.href = objectUrl;
        a.click();
        URL.revokeObjectURL(objectUrl);
      })
      .catch((e) => setActionError(String(e)));
  };

  const handleRestore = async () => {
    if (!restoreTarget) return;
    setRestoring(true);
    setActionError("");
    try {
      // Fetch the backup file
      const token = getToken();
      const resp = await fetch(`/api/backup/${encodeURIComponent(restoreTarget)}`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!resp.ok) throw new Error(`Failed to download backup: HTTP ${resp.status}`);
      const blob = await resp.blob();

      // POST to restore
      const restoreResp = await fetch("/api/restore", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/octet-stream",
        },
        body: blob,
      });
      if (!restoreResp.ok) {
        const text = await restoreResp.text().catch(() => "");
        throw new Error(text || `HTTP ${restoreResp.status}`);
      }
      refetch();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setRestoring(false);
      setRestoreTarget(null);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    setActionError("");
    try {
      await apiDelete(`/api/backup/${encodeURIComponent(deleteTarget)}`);
      toast.success(t("backups.deleted"));
      refetch();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setDeleting(false);
      setDeleteTarget(null);
    }
  };

  const mutating = createBackup.isPending || restoring || deleting;
  const combinedError =
    (error ? `${error}` : "") ||
    (createBackup.error ? `${createBackup.error}` : "") ||
    actionError;

  return (
    <div className="flex-1 overflow-y-auto p-4 md:p-6 lg:p-8 selection:bg-primary/20">
        <div className="mb-8 flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div>
            <h2 className="font-display text-lg font-bold tracking-tight text-foreground">
              {t("backups.title")}
            </h2>
            <p className="text-sm text-muted-foreground mt-1">
              {t("backups.subtitle")}
            </p>
          </div>
          <div className="grid grid-cols-2 md:flex gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => refetch()}
              disabled={isLoading || mutating}
            >
              <RefreshCw className={`mr-2 h-4 w-4 ${isLoading ? "animate-spin" : ""}`} />
              {t("common.refresh")}
            </Button>
            <Button
              size="sm"
              onClick={() => createBackup.mutate()}
              disabled={isLoading || mutating}
            >
              <Plus className="mr-2 h-4 w-4" />
              {createBackup.isPending ? t("backups.creating") : t("backups.create")}
            </Button>
          </div>
        </div>

        {combinedError && <ErrorBanner error={combinedError} />}

        {backups.length === 0 ? (
          <div className="flex h-40 items-center justify-center rounded-xl border border-dashed border-border bg-muted/10">
            <p className="font-mono text-sm text-muted-foreground/40 uppercase tracking-wider">
              {t("backups.empty")}
            </p>
          </div>
        ) : (
          <div className="space-y-3 pb-8">
            {backups.map((b) => (
              <div
                key={b.filename}
                className="group relative flex flex-col md:flex-row md:items-center gap-4 neu-flat p-4 transition-all hover:border-primary/20"
              >
                <div className="flex items-center gap-3 md:min-w-[300px]">
                  <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 border border-primary/20">
                    <Archive className="h-5 w-5 text-primary" />
                  </div>
                  <div className="flex flex-col min-w-0">
                    <span className="break-all font-mono text-sm font-bold text-foreground group-hover:text-primary transition-colors">
                      {b.filename}
                    </span>
                    <span className="font-mono text-xs text-muted-foreground/40 tabular-nums">
                      {formatDate(b.created_at, locale)}
                    </span>
                  </div>
                </div>

                <div className="flex flex-1 items-center gap-3">
                  <span className="font-mono text-sm text-muted-foreground tabular-nums">
                    {formatBytes(b.size_bytes)}
                  </span>
                </div>

                <div className="grid grid-cols-2 md:flex md:items-center md:justify-end gap-2 border-t border-border/50 pt-3 md:border-0 md:pt-0 shrink-0">
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground hover:text-primary hover:bg-primary/10"
                    onClick={() => handleDownload(b.filename)}
                    disabled={mutating}
                  >
                    <Download className="mr-2 h-4 w-4" />
                    {t("backups.download")}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground hover:text-destructive hover:bg-destructive/10"
                    onClick={() => setRestoreTarget(b.filename)}
                    disabled={mutating}
                  >
                    <RotateCcw className="mr-2 h-4 w-4" />
                    {restoring ? t("backups.restoring") : t("backups.restore")}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-muted-foreground hover:text-destructive hover:bg-destructive/10"
                    onClick={() => setDeleteTarget(b.filename)}
                    disabled={mutating}
                  >
                    <Trash2 className="mr-2 h-4 w-4" />
                    {t("common.delete")}
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}

      <ConfirmDialog
        open={!!restoreTarget}
        onClose={() => setRestoreTarget(null)}
        onConfirm={handleRestore}
        title={t("backups.restore_title")}
        description={t("backups.restore_description", { filename: restoreTarget ?? "" })}
      />
      <ConfirmDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        title={t("backups.delete_title")}
        description={t("backups.delete_description", { filename: deleteTarget ?? "" })}
      />
    </div>
  );
}

"use client";

import { useEffect, useState, useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { apiGet, apiPatch, apiDelete } from "@/lib/api";
import { useMemoryStats, qk } from "@/lib/queries";
import { useTranslation } from "@/hooks/use-translation";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ErrorBanner } from "@/components/ui/error-banner";
import { Skeleton } from "@/components/ui/skeleton";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Brain, Search, Trash2, Pin, PinOff, ChevronLeft, ChevronRight, ChevronDown, X } from "lucide-react";
import type { MemoryDocument } from "@/types/api";

// ── Lazy-load full document content ──

function DocumentContent({ id, onCollapse }: { id: string; onCollapse: () => void }) {
  const { t } = useTranslation();
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    apiGet<{ content: string }>(`/api/memory/documents/${id}`)
      .then((res) => setContent(res.content))
      .finally(() => setLoading(false));
  }, [id]);

  if (loading) return <Skeleton className="h-24 w-full mt-2" />;

  return (
    <div className="mt-2 text-xs leading-relaxed text-muted-foreground whitespace-pre-wrap border-t pt-2 relative group">
      {content}
      <Button
        variant="ghost"
        size="sm"
        className="absolute top-2 right-0 h-6 px-2 opacity-0 group-hover:opacity-100 transition-opacity"
        onClick={onCollapse}
      >
        <ChevronDown className="h-3 w-3 rotate-180" />
      </Button>
    </div>
  );
}

export default function MemoryPage() {
  const { t, locale } = useTranslation();
  const qc = useQueryClient();
  const { data: stats } = useMemoryStats();

  const [chunks, setChunks] = useState<MemoryDocument[]>([]);
  const [query, setQuery] = useState("");
  const [offset, setOffset] = useState(0);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);

  const limit = 20;

  const fetchChunks = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const params = new URLSearchParams({
        limit: limit.toString(),
        offset: offset.toString(),
      });
      if (query) params.append("query", query);

      const res = await apiGet<{ documents: MemoryDocument[]; total: number }>(`/api/memory/documents?${params.toString()}`);
      setChunks(res.documents);
    } catch (err: any) {
      setError(err.message || "Failed to fetch memory");
    } finally {
      setLoading(false);
    }
  }, [offset, query]);

  useEffect(() => {
    fetchChunks();
  }, [fetchChunks]);

  const doDelete = async () => {
    if (!deleteTarget) return;
    try {
      await apiDelete(`/api/memory/documents/${deleteTarget}`);
      setChunks(chunks.filter((c) => c.id !== deleteTarget));
      setDeleteTarget(null);
      qc.invalidateQueries({ queryKey: qk.memoryStats });
    } catch (err: any) {
      setError(err.message);
    }
  };

  const togglePin = async (doc: MemoryDocument) => {
    try {
      const newPinned = !doc.pinned;
      await apiPatch(`/api/memory/documents/${doc.id}`, { pinned: newPinned });
      setChunks(chunks.map((c) => (c.id === doc.id ? { ...c, pinned: newPinned } : c)));
      qc.invalidateQueries({ queryKey: qk.memoryStats });
    } catch (err: any) {
      setError(err.message);
    }
  };

  const next = () => setOffset(offset + limit);
  const prev = () => setOffset(Math.max(0, offset - limit));

  // Reset pagination on filter change
  useEffect(() => {
    setOffset(0);
  }, [query]);

  return (
    <div className="flex flex-col h-full p-4 md:p-8 max-w-6xl mx-auto w-full overflow-hidden">
      <div className="mb-8 flex flex-col md:flex-row md:items-end justify-between gap-4 shrink-0">
        <div className="flex flex-col gap-1">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-xl bg-primary/10 text-primary">
              <Brain className="h-6 w-6" />
            </div>
            <h2 className="font-display text-lg font-bold tracking-tight text-foreground">{t("memory.title")}</h2>
          </div>
          <span className="text-xs text-muted-foreground ml-11">
            {t("memory.subtitle")}
          </span>
        </div>

        <div className="flex flex-wrap items-stretch gap-3 md:gap-6">
          {stats && (
            <div className="flex items-center gap-4 px-4 py-2 bg-muted/30 rounded-xl border border-border/50">
              <div className="flex flex-col">
                <span className="text-xs text-muted-foreground">{t("memory.documents")}</span>
                <span className="font-mono text-sm font-bold text-foreground">{stats.total.toLocaleString()}</span>
              </div>
              <div className="w-[1px] h-8 bg-border/50" />
              <div className="flex flex-col">
                <span className="text-xs text-muted-foreground">{t("memory.pinned")}</span>
                <span className="font-mono text-sm font-bold text-foreground">{stats.pinned.toLocaleString()}</span>
              </div>
            </div>
          )}
        </div>
      </div>

      {error && <ErrorBanner error={error} className="mb-4 shrink-0" />}

      <div className="flex flex-col flex-1 min-h-0">
          {/* Search + Create */}
          <div className="mb-6 flex gap-2 shrink-0">
            <div className="relative flex-1 min-w-0">
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground/70" />
              <Input
                placeholder={t("memory.search_placeholder")}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                className="pl-9 bg-card border-border/50"
              />
              {query && (
                <button
                  onClick={() => setQuery("")}
                  className="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
                >
                  <X className="h-3 w-3" />
                </button>
              )}
            </div>
          </div>

          {/* Table View */}
          <div className="flex-1 min-h-0 overflow-y-auto pr-1 -mr-1 custom-scrollbar">
            {loading && chunks.length === 0 ? (
              <div className="space-y-3">
                {[1, 2, 3].map((i) => (
                  <Skeleton key={i} className="h-20 w-full rounded-xl" />
                ))}
              </div>
            ) : chunks.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-20 text-muted-foreground border-2 border-dashed rounded-3xl bg-muted/10">
                <Brain className="h-10 w-10 mb-4 opacity-20" />
                <p className="text-sm">{t("memory.nothing_found")}</p>
              </div>
            ) : (
              <div className="grid gap-3">
                {chunks.map((doc) => (
                  <div
                    key={doc.id}
                    className="group relative flex flex-col p-4 bg-card hover:bg-muted/30 border border-border/50 rounded-2xl transition-all duration-200 shadow-sm"
                  >
                    <div className="flex items-start justify-between gap-4">
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2 mb-1">
                          <h3 className="font-semibold text-sm truncate text-foreground group-hover:text-primary transition-colors">
                            {doc.source || t("memory.untitled")}
                          </h3>
                          {doc.pinned && (
                            <Badge variant="secondary" className="h-5 px-1.5 bg-primary/10 text-primary border-none">
                              <Pin className="h-3 w-3" />
                            </Badge>
                          )}
                        </div>
                        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                          <span className="text-[10px] uppercase tracking-wider font-bold text-muted-foreground/60">
                            ID: {doc.id.split("-")[0]}
                          </span>
                          <span className="text-[10px] text-muted-foreground/60">
                            {doc.created_at ? new Date(doc.created_at).toLocaleDateString(locale) : ""}
                          </span>
                          {doc.scope === "shared" && (
                            <Badge variant="secondary" className="h-4 text-[9px] px-1 py-0 bg-blue-500/10 text-blue-400 border-none">
                              shared
                            </Badge>
                          )}
                          {doc.category && (
                            <Badge variant="outline" className="h-4 text-[9px] px-1 py-0 border-muted-foreground/30 text-muted-foreground/80">
                              {doc.category}
                            </Badge>
                          )}
                        </div>
                      </div>

                      <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => setExpandedId(expandedId === doc.id ? null : doc.id)}
                          className="h-7 text-xs px-2"
                        >
                          {expandedId === doc.id ? t("common.collapse") : t("memory.show_full_document")}
                        </Button>
                        <Button variant="ghost" size="sm" onClick={() => togglePin(doc)} className="h-7 w-7 p-0">
                          {doc.pinned ? <PinOff className="h-3.5 w-3.5" /> : <Pin className="h-3.5 w-3.5" />}
                        </Button>
                        <Button variant="ghost" size="sm" onClick={() => setDeleteTarget(doc.id)} className="h-7 text-xs px-2 text-destructive hover:bg-destructive/10">
                          <Trash2 className="h-3 w-3 mr-1.5" /> {t("common.delete")}
                        </Button>
                      </div>
                    </div>

                    {expandedId === doc.id && (
                      <DocumentContent id={doc.id} onCollapse={() => setExpandedId(null)} />
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* Pagination */}
          {chunks.length > 0 && (
            <div className="mt-6 flex justify-center gap-3 shrink-0">
              <Button variant="outline" size="sm" onClick={prev} disabled={offset === 0 || loading} className="text-xs w-24">
                <ChevronLeft className="h-3.5 w-3.5 mr-1" /> {t("common.back")}
              </Button>
              <Button variant="outline" size="sm" onClick={next} disabled={chunks.length < limit || loading} className="text-xs w-24">
                {t("common.forward")} <ChevronRight className="h-3.5 w-3.5 ml-1" />
              </Button>
            </div>
          )}
        </div>

      <ConfirmDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={doDelete}
        title={t("memory.delete_chunk_title")}
        description={t("memory.delete_chunk_description")}
      />
    </div>
  );
}

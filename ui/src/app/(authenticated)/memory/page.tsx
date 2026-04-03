"use client";

import { useEffect, useState, useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { apiGet, apiPost, apiPatch, apiDelete } from "@/lib/api";
import { useMemoryStats, qk } from "@/lib/queries";
import { useTranslation } from "@/hooks/use-translation";
import { ErrorBanner } from "@/components/ui/error-banner";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { formatDate } from "@/lib/format";
import { Textarea } from "@/components/ui/textarea";
import { EmptyState } from "@/components/ui/empty-state";
import { Brain, Plus, Search, Trash2, Pin, PinOff, ChevronLeft, ChevronRight, ChevronDown, X } from "lucide-react";
import type { MemoryDocument } from "@/types/api";

// ── Lazy-load full document content ──

function DocumentContent({ id, onCollapse }: { id: string; onCollapse: () => void }) {
  const { t } = useTranslation();
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    apiGet<{ content: string }>(`/api/memory/documents/${id}`)
      .then((data) => setContent(data.content))
      .catch((e) => { console.warn("[memory] chunk load failed:", e); setContent("Failed to load"); })
      .finally(() => setLoading(false));
  }, [id]);

  if (loading) return <Skeleton className="h-24 w-full rounded-md" />;

  return (
    <div className="mb-3 rounded-md bg-muted/20 border border-border/50 px-3 py-2.5">
      <p className="text-sm leading-relaxed text-foreground/90 whitespace-pre-wrap break-words overflow-hidden max-w-full max-h-[500px] overflow-y-auto">
        {content}
      </p>
      <div className="mt-2 flex items-center gap-1 text-xs text-primary cursor-pointer" onClick={onCollapse}>
        <ChevronDown className="h-3.5 w-3.5 rotate-180" />
        {t("memory.collapse")}
      </div>
    </div>
  );
}

// ── Main page ──

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
  const [searchMode, setSearchMode] = useState<string>("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [creating, setCreating] = useState(false);
  const [createContent, setCreateContent] = useState("");
  const [createPinned, setCreatePinned] = useState(false);
  const [createSaving, setCreateSaving] = useState(false);
  const limit = 20;

  const search = useCallback(async (q: string, off: number) => {
    setLoading(true);
    try {
      const params = new URLSearchParams({ limit: String(limit), offset: String(off) });
      if (q.trim()) params.set("query", q.trim());
      const data = await apiGet<{ documents: MemoryDocument[]; total?: number; search_mode?: string }>(`/api/memory/documents?${params}`);
      setChunks(data.documents || []);
      setSearchMode(data.search_mode || "");
      setExpanded(new Set());
      setError("");
    } catch (e) {
      setError(`${e}`);
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    search("", 0);
  }, [search]);

  const doSearch = () => { setOffset(0); search(query, 0); };
  const prev = () => { const o = Math.max(0, offset - limit); setOffset(o); search(query, o); };
  const next = () => { const o = offset + limit; setOffset(o); search(query, o); };

  const handleTogglePin = async (id: string) => {
    const doc = chunks.find((c) => c.id === id);
    if (!doc) return;
    try {
      await apiPatch(`/api/memory/documents/${id}`, { pinned: !doc.pinned });
      search(query, offset);
      qc.invalidateQueries({ queryKey: qk.memoryStats });
    } catch (e) { setError(`${e}`); }
  };

  const doDelete = async () => {
    if (!deleteTarget) return;
    try {
      await apiDelete(`/api/memory/documents/${deleteTarget}`);
      search(query, offset);
      qc.invalidateQueries({ queryKey: qk.memoryStats });
    } catch (e) { setError(`${e}`); }
    setDeleteTarget(null);
  };

  const doCreate = async () => {
    if (!createContent.trim()) return;
    setCreateSaving(true);
    try {
      await apiPost("/api/memory", { content: createContent, pinned: createPinned });
      setCreating(false);
      setCreateContent("");
      setCreatePinned(false);
      search(query, offset);
      qc.invalidateQueries({ queryKey: qk.memoryStats });
    } catch (e) {
      setError(`${e}`);
    } finally {
      setCreateSaving(false);
    }
  };


  return (
    <div className="flex-1 flex flex-col p-4 md:p-6 lg:p-8 selection:bg-primary/20 overflow-y-auto">
      {/* Header */}
      <div className="mb-6 flex flex-col md:flex-row md:items-end justify-between gap-4 shrink-0">
        <div className="flex flex-col gap-1">
          <h2 className="font-display text-lg font-bold tracking-tight text-foreground">{t("memory.title")}</h2>
          <span className="text-sm text-muted-foreground">
            {t("memory.subtitle")}
          </span>
        </div>

        <div className="flex flex-wrap items-stretch gap-3 md:gap-6">
          {stats && (
            <div className="flex flex-wrap gap-3 md:gap-6 bg-muted/20 px-4 py-2 rounded-lg border border-border">
              <div className="flex flex-col">
                <span className="text-xs text-muted-foreground">{t("memory.documents")}</span>
                <span className="font-mono text-sm font-bold text-foreground">{stats.total.toLocaleString()}</span>
              </div>
              <div className="w-px bg-border/50" />
              <div className="flex flex-col">
                <span className="text-xs text-muted-foreground">{t("memory.total_chunks")}</span>
                <span className="font-mono text-sm font-bold text-foreground">{(stats.total_chunks ?? 0).toLocaleString()}</span>
              </div>
              <div className="w-px bg-border/50" />
              <div className="flex flex-col">
                <span className="text-xs text-muted-foreground">{t("memory.pinned")}</span>
                <span className="font-mono text-sm font-bold text-primary">{stats.pinned}</span>
              </div>
            </div>
          )}
        </div>
      </div>

      {error && <ErrorBanner error={error} className="mb-4 shrink-0" />}

      {/* Search + Create */}
      <div className="mb-6 flex gap-2">
        <div className="relative flex-1 min-w-0">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground/70" />
          <Input
            placeholder={t("memory.search_placeholder")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") doSearch(); }}
            className="pl-10 h-10 text-sm bg-card/50 border-border focus:border-primary/50"
          />
        </div>
        <Button onClick={doSearch} disabled={loading} size="lg" className="font-semibold shrink-0">
          {t("memory.search_button")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-10 px-3 border-primary/30 text-primary hover:bg-primary/10 shrink-0"
          onClick={() => setCreating((v) => !v)}
          aria-label={creating ? t("common.cancel") : t("common.create")}
        >
          {creating ? <X className="h-4 w-4" /> : <Plus className="h-4 w-4" />}
        </Button>
      </div>

      {creating && (
        <div className="mb-6 rounded-xl border border-primary/30 bg-primary/5 p-4 space-y-3">
          <Textarea
            value={createContent}
            onChange={(e) => setCreateContent(e.target.value)}
            placeholder={t("memory.create_placeholder")}
            className="min-h-[80px] md:min-h-[100px] text-sm bg-card/50 border-border"
            autoFocus
          />
          <div className="flex items-center justify-between">
            <label className="flex items-center gap-2 text-xs text-muted-foreground cursor-pointer">
              <input
                type="checkbox"
                checked={createPinned}
                onChange={(e) => setCreatePinned(e.target.checked)}
                className="rounded border-border accent-primary"
                aria-label={t("memory.pin")}
              />
              <Pin className="h-3 w-3" /> {t("memory.pin")}
            </label>
            <div className="flex gap-2">
              <Button variant="ghost" size="sm" onClick={() => setCreating(false)} className="text-xs">
                {t("common.cancel")}
              </Button>
              <Button size="sm" onClick={doCreate} disabled={createSaving || !createContent.trim()} className="text-xs">
                {t("common.create")}
              </Button>
            </div>
          </div>
        </div>
      )}

      {searchMode && query.trim() && (
        <div className="mb-4 flex items-center gap-2">
          <Badge variant="outline" className="text-[10px] border-border text-muted-foreground">
            {searchMode === "semantic" ? t("memory.search_mode_semantic") : searchMode === "fts" ? t("memory.search_mode_fts") : t("memory.search_mode_text")}
          </Badge>
        </div>
      )}

      {/* Document list */}
      {chunks.length === 0 ? (
        <EmptyState icon={Brain} text={t("memory.nothing_found")} height="h-48" />
      ) : (
        <div className="space-y-3">
          {chunks.map((doc) => (
            <div key={doc.id} className={`neu-flat p-4 transition-colors ${doc.pinned ? "border-l-[3px] border-l-primary bg-primary/5 border-primary/20" : "hover:border-primary/30"}`}>
              <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                <div className="flex flex-wrap items-center gap-2">
                  {doc.pinned && <Badge className="bg-primary text-primary-foreground text-[10px] px-1.5 py-0">{t("memory.pinned_badge")}</Badge>}
                  <span className="font-mono text-sm font-medium text-foreground">
                    {doc.source || t("memory.untitled")}
                  </span>
                  <Badge variant="outline" className="font-mono text-[10px] border-border text-muted-foreground">
                    {doc.chunks_count} {doc.chunks_count === 1 ? t("memory.chunk") : t("memory.chunks")}
                  </Badge>
                  {doc.total_chars != null && (
                    <span className="text-[10px] text-muted-foreground">
                      {(doc.total_chars / 1000).toFixed(1)}k
                    </span>
                  )}
                  {doc.similarity != null && doc.similarity > 0 && (
                    <span className="font-mono text-[10px] text-muted-foreground tabular-nums">
                      {t("memory.similarity", { value: doc.similarity.toFixed(3) })}
                    </span>
                  )}
                </div>
                {doc.accessed_at && (
                  <span className="text-[11px] text-muted-foreground">
                    {formatDate(doc.accessed_at, locale)}
                  </span>
                )}
              </div>

              {expanded.has(doc.id) ? (
                <DocumentContent id={doc.id} onCollapse={() => setExpanded((prev) => { const n = new Set(prev); n.delete(doc.id); return n; })} />
              ) : (
                <div
                  className="mb-3 rounded-md bg-muted/20 border border-border/50 px-3 py-2.5 cursor-pointer hover:bg-muted/30 transition-colors"
                  onClick={() => setExpanded((prev) => { const n = new Set(prev); n.add(doc.id); return n; })}
                >
                  <p className="text-sm leading-relaxed text-foreground/90 whitespace-pre-wrap break-words overflow-hidden max-w-full">
                    {doc.preview ? doc.preview + "\u2026" : "\u2014"}
                  </p>
                  <div className="mt-2 flex items-center gap-1 text-xs text-primary">
                    <ChevronDown className="h-3.5 w-3.5" />
                    {t("memory.show_full_document")}
                  </div>
                </div>
              )}

              <div className="flex flex-wrap justify-end gap-2">
                <Button variant="ghost" size="sm" onClick={() => handleTogglePin(doc.id)} className={`h-7 text-xs px-2 ${doc.pinned ? "text-muted-foreground hover:text-foreground" : "text-primary hover:bg-primary/10"}`}>
                  {doc.pinned ? <><PinOff className="h-3 w-3 mr-1.5" /> {t("memory.unpin")}</> : <><Pin className="h-3 w-3 mr-1.5" /> {t("memory.pin")}</>}
                </Button>
                <Button variant="ghost" size="sm" onClick={() => setDeleteTarget(doc.id)} className="h-7 text-xs px-2 text-destructive hover:bg-destructive/10">
                  <Trash2 className="h-3 w-3 mr-1.5" /> {t("common.delete")}
                </Button>
              </div>
            </div>
          ))}
        </div>
      )}

      {chunks.length > 0 && (
        <div className="mt-6 flex justify-center gap-3">
          <Button variant="outline" size="sm" onClick={prev} disabled={offset === 0 || loading} className="text-xs w-24">
            <ChevronLeft className="h-3.5 w-3.5 mr-1" /> {t("common.back")}
          </Button>
          <Button variant="outline" size="sm" onClick={next} disabled={chunks.length < limit || loading} className="text-xs w-24">
            {t("common.forward")} <ChevronRight className="h-3.5 w-3.5 ml-1" />
          </Button>
        </div>
      )}

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

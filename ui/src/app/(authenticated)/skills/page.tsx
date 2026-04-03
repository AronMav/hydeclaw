"use client";

import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { apiGet, apiDelete, apiPut } from "@/lib/api";
import { useSkills, qk } from "@/lib/queries";
import { useTranslation } from "@/hooks/use-translation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { ErrorBanner } from "@/components/ui/error-banner";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import {
  BookOpen, Wrench, Zap, Trash2, RefreshCw, Tag,
  Plus, Pencil, ArrowLeft, Save, FileText,
} from "lucide-react";
import { toast } from "sonner";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel,
  AlertDialogContent, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import type { SkillEntry } from "@/types/api";

interface SkillForm {
  name: string;
  description: string;
  triggers: string;
  tools_required: string;
  priority: string;
  instructions: string;
}

const EMPTY_FORM: SkillForm = {
  name: "",
  description: "",
  triggers: "",
  tools_required: "",
  priority: "0",
  instructions: "",
};

export default function SkillsPage() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { data, isLoading: loading, error } = useSkills();
  const skills: SkillEntry[] = Array.isArray(data) ? data : [];
  const [deletePending, setDeletePending] = useState<string | null>(null);
  const [deleteConfirm, setDeleteConfirm] = useState<string | null>(null);

  const [showForm, setShowForm] = useState(false);
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [form, setForm] = useState<SkillForm>(EMPTY_FORM);
  const [saving, setSaving] = useState(false);

  const handleDelete = async (skillName: string) => {
    setDeletePending(skillName);
    try {
      await apiDelete(`/api/skills/${encodeURIComponent(skillName)}`);
      qc.invalidateQueries({ queryKey: qk.skills });
      toast.success(t("skills.skill_deleted", { name: skillName }));
    } catch (e) {
      toast.error(t("skills.skill_delete_error", { error: String(e) }));
    } finally {
      setDeletePending(null);
    }
  };

  const openNew = () => {
    setForm(EMPTY_FORM);
    setEditingKey(null);
    setShowForm(true);
  };

  const openEdit = async (skill: SkillEntry) => {
    try {
      const data = await apiGet<{
        name: string;
        content: string;
        description?: string;
        triggers?: string[];
        tools_required?: string[];
        priority?: number;
        instructions?: string;
      }>(`/api/skills/${encodeURIComponent(skill.name)}`);
      setForm({
        name: skill.name,
        description: data.description ?? skill.description,
        triggers: (data.triggers ?? skill.triggers).join("\n"),
        tools_required: (data.tools_required ?? skill.tools_required).join("\n"),
        priority: String(data.priority ?? skill.priority ?? 0),
        instructions: data.instructions ?? "",
      });
      setEditingKey(skill.name);
      setShowForm(true);
    } catch (e) {
      toast.error(t("skills.skill_load_error", { error: String(e) }));
    }
  };

  const handleSave = async () => {
    if (!form.name.trim()) { toast.error(t("skills.field_name_required")); return; }
    setSaving(true);
    try {
      await apiPut(`/api/skills/${encodeURIComponent(form.name.trim())}`, {
        description: form.description.trim(),
        triggers: form.triggers.split("\n").map((t) => t.trim()).filter(Boolean),
        tools_required: form.tools_required.split("\n").map((t) => t.trim()).filter(Boolean),
        priority: parseInt(form.priority || "0", 10),
        instructions: form.instructions,
      });
      toast.success(editingKey ? t("skills.skill_updated", { name: form.name }) : t("skills.skill_created", { name: form.name }));
      setShowForm(false);
      qc.invalidateQueries({ queryKey: qk.skills });
    } catch (e) {
      toast.error(t("skills.skill_save_error", { error: String(e) }));
    } finally {
      setSaving(false);
    }
  };

  // ── Form view ──────────────────────────────────────────────────────────────

  if (showForm) {
    return (
      <div className="flex-1 overflow-y-auto p-4 md:p-6 lg:p-8 selection:bg-primary/20">
        <div className="mx-auto max-w-3xl">
          <div className="mb-8 flex items-center gap-3">
            <Button variant="outline" size="sm" onClick={() => setShowForm(false)}>
              <ArrowLeft className="h-3.5 w-3.5" />
              {t("common.back")}
            </Button>
            <div>
              <h2 className="font-display text-lg font-bold tracking-tight text-foreground">
                {editingKey ? t("skills.editing", { name: form.name }) : t("skills.new_skill_title")}
              </h2>
              <span className="text-sm text-muted-foreground">
                {editingKey ? t("skills.editing_subtitle") : t("skills.new_skill_subtitle")}
              </span>
            </div>
          </div>

          <div className="rounded-xl border border-border/60 bg-card/50 p-6 space-y-5">
            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-medium text-muted-foreground">
                {t("skills.field_name")} <span className="text-destructive">*</span>
              </label>
              <Input
                type="text"
                value={form.name}
                onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
                disabled={!!editingKey}
                placeholder="e.g. research-task"
                className="font-mono max-w-md"
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-medium text-muted-foreground">{t("skills.field_description")}</label>
              <Input
                type="text"
                value={form.description}
                onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
                placeholder={t("skills.description_placeholder")}
              />
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-medium text-muted-foreground">
                  {t("skills.field_triggers")} <span className="text-muted-foreground/50 font-normal">({t("skills.triggers_hint")})</span>
                </label>
                <Textarea
                  value={form.triggers}
                  onChange={(e) => setForm((f) => ({ ...f, triggers: e.target.value }))}
                  placeholder={"research\ninvestigate\nfind information"}
                  rows={4}
                  className="resize-none font-mono"
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <label className="text-xs font-medium text-muted-foreground">
                  {t("skills.field_tools_required")} <span className="text-muted-foreground/50 font-normal">({t("skills.tools_hint")})</span>
                </label>
                <Textarea
                  value={form.tools_required}
                  onChange={(e) => setForm((f) => ({ ...f, tools_required: e.target.value }))}
                  placeholder={"web_search\nmemory\nworkspace_write"}
                  rows={4}
                  className="resize-none font-mono"
                />
              </div>
            </div>

            <div className="flex flex-col gap-1.5 max-w-48">
              <label className="text-xs font-medium text-muted-foreground">{t("skills.field_priority")}</label>
              <Input
                type="number"
                value={form.priority}
                onChange={(e) => setForm((f) => ({ ...f, priority: e.target.value }))}
                min={0}
              />
              <p className="text-xs text-muted-foreground/60">{t("skills.priority_hint")}</p>
            </div>

            <div className="flex flex-col gap-1.5">
              <label className="text-xs font-medium text-muted-foreground">
                {t("skills.field_instructions")} <span className="text-muted-foreground/50 font-normal">({t("skills.instructions_hint")})</span>
              </label>
              <Textarea
                value={form.instructions}
                onChange={(e) => setForm((f) => ({ ...f, instructions: e.target.value }))}
                placeholder={"## Step 1\nDo this first...\n\n## Step 2\nThen do this..."}
                rows={14}
                className="resize-y font-mono"
              />
            </div>
          </div>

          <div className="mt-4 flex items-center justify-end gap-3">
            <Button variant="ghost" onClick={() => setShowForm(false)}>
              {t("common.cancel")}
            </Button>
            <Button onClick={handleSave} disabled={saving}>
              <Save className="h-4 w-4" />
              {saving ? t("skills.saving") : t("skills.save_skill")}
            </Button>
          </div>
        </div>
      </div>
    );
  }

  // ── List view ──────────────────────────────────────────────────────────────

  return (
    <div className="flex flex-col gap-8 p-4 md:p-6 lg:p-8 selection:bg-primary/20">
      {/* Header */}
      <div className="flex flex-col md:flex-row md:items-start justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h2 className="font-display text-lg font-bold tracking-tight">{t("skills.title")}</h2>
          <span className="text-sm text-muted-foreground">{t("skills.subtitle")}</span>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => qc.invalidateQueries({ queryKey: qk.skills })}
            disabled={loading}
          >
            <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
            {t("common.refresh")}
          </Button>
          <Button size="sm" onClick={openNew}>
            <Plus className="h-3.5 w-3.5" />
            {t("skills.new_skill")}
          </Button>
        </div>
      </div>

      {error && <ErrorBanner error={String(error)} />}

      {loading ? (
        <div className="space-y-4">
          {[1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-32 rounded-xl border border-border bg-muted/20" />
          ))}
        </div>
      ) : skills.length === 0 ? (
        <EmptyState icon={BookOpen} text={t("skills.no_skills")} hint={
          <p className="text-xs text-muted-foreground/60 mt-1">
            {t("skills.no_skills_hint_prefix")}<span className="font-mono">skill(action=&quot;create&quot;)</span>{t("skills.no_skills_hint_middle")}
            <Button variant="link" onClick={openNew} className="p-0 h-auto">{t("skills.no_skills_hint_link")}</Button>
          </p>
        } />
      ) : (
        <div className="space-y-3">
          {skills.map((skill) => {
            const isPending = deletePending === skill.name;

            return (
              <div key={skill.name} className="rounded-xl border border-border/60 bg-card/50 p-5 space-y-4">
                {/* Header */}
                <div className="flex items-start gap-3">
                  <div className="flex items-center justify-center h-10 w-10 rounded-lg bg-primary/10 border border-primary/20 shrink-0">
                    <BookOpen className="h-4.5 w-4.5 text-primary" />
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-sm font-semibold text-foreground truncate">
                        {skill.name}
                      </span>
                      {skill.priority > 0 && (
                        <Badge variant="secondary" className="text-[10px] px-1.5 py-0 font-mono shrink-0">
                          p:{skill.priority}
                        </Badge>
                      )}
                    </div>
                    {skill.description && (
                      <p className="text-xs text-muted-foreground mt-0.5 line-clamp-2">{skill.description}</p>
                    )}
                  </div>
                </div>

                {/* Triggers */}
                {skill.triggers.length > 0 && (
                  <div className="flex flex-wrap items-start gap-2">
                    <div className="flex items-center gap-1.5 shrink-0 pt-0.5">
                      <Zap className="h-3 w-3 text-warning" />
                      <span className="text-xs text-muted-foreground font-medium">{t("skills.triggers_label")}</span>
                    </div>
                    <div className="flex flex-wrap gap-1.5">
                      {skill.triggers.map((tr) => (
                        <span key={tr} className="inline-flex items-center gap-1 rounded-md border border-border/60 bg-muted/30 px-2 py-0.5 text-xs text-foreground/80">
                          <Tag className="h-2.5 w-2.5 text-muted-foreground" />
                          {tr}
                        </span>
                      ))}
                    </div>
                  </div>
                )}

                {/* Tools required */}
                {skill.tools_required.length > 0 && (
                  <div className="flex flex-wrap items-start gap-2">
                    <div className="flex items-center gap-1.5 shrink-0 pt-0.5">
                      <Wrench className="h-3 w-3 text-primary" />
                      <span className="text-xs text-muted-foreground font-medium">{t("skills.tools_label")}</span>
                    </div>
                    <div className="flex flex-wrap gap-1.5">
                      {skill.tools_required.map((tr) => (
                        <span key={tr} className="inline-flex items-center rounded-md border border-primary/20 bg-primary/5 px-2 py-0.5 text-xs font-mono text-primary/80">
                          {tr}
                        </span>
                      ))}
                    </div>
                  </div>
                )}

                {/* Footer: instructions size + actions */}
                <div className="flex items-center justify-between pt-1 border-t border-border/30">
                  <div className="flex items-center gap-1.5">
                    <FileText className="h-3 w-3 text-muted-foreground/50" />
                    <span className="text-xs text-muted-foreground/60">
                      {t("skills.instructions_size")} {t("skills.instructions_chars", { count: skill.instructions_len.toLocaleString() })}
                    </span>
                  </div>
                  <div className="flex items-center gap-1">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => openEdit(skill)}
                      className="h-7 text-xs"
                    >
                      <Pencil className="h-3 w-3" />
                      {t("common.edit")}
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={isPending}
                      onClick={() => setDeleteConfirm(skill.name)}
                      className="h-7 text-xs text-destructive hover:text-destructive"
                    >
                      <Trash2 className="h-3 w-3" />
                      {t("common.delete")}
                    </Button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <AlertDialog open={!!deleteConfirm} onOpenChange={(o) => !o && setDeleteConfirm(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("skills.delete_skill_confirm_title")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("skills.delete_skill_confirm_description", { name: deleteConfirm ?? "" })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (deleteConfirm) {
                  handleDelete(deleteConfirm);
                  setDeleteConfirm(null);
                }
              }}
            >
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

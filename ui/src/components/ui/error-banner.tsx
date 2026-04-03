"use client";

import { useTranslation } from "@/hooks/use-translation";

interface ErrorBannerProps {
  error: string;
  className?: string;
}

export function ErrorBanner({ error, className }: ErrorBannerProps) {
  const { t } = useTranslation();
  if (!error) return null;
  return (
    <div className={`mb-8 flex items-center gap-3 rounded-lg border border-destructive/30 bg-destructive/10 p-4 ${className ?? ""}`}>
      <div className="h-2 w-2 shrink-0 rounded-full bg-destructive" />
      <p className="text-sm font-medium text-destructive">{t("common.error_prefix", { error })}</p>
    </div>
  );
}

"use client";

import { Button } from "@/components/ui/button";
import { useTranslation } from "@/hooks/use-translation";

export default function AuthenticatedError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-4 p-8">
      <p className="font-mono text-sm text-destructive">{error.message}</p>
      <Button onClick={reset} variant="outline" size="sm">
        {t("error.retry")}
      </Button>
    </div>
  );
}

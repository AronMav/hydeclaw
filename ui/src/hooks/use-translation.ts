"use client";

import { useCallback } from "react";
import { useLanguageStore } from "@/stores/language-store";
import { getTranslations } from "@/i18n";
import type { TranslationKey } from "@/i18n/types";

type InterpolationValues = Record<string, string | number>;

export function useTranslation() {
  const locale = useLanguageStore((s) => s.locale);
  const translations = getTranslations(locale);

  const t = useCallback(
    (key: TranslationKey, values?: InterpolationValues): string => {
      let text = translations[key] ?? key;
      if (values) {
        for (const [k, v] of Object.entries(values)) {
          text = text.replaceAll(`{{${k}}}`, String(v));
        }
      }
      return text;
    },
    [translations],
  );

  return { t, locale };
}

"use client";

import { useEffect } from "react";
import { useLanguageStore } from "@/stores/language-store";

export function LanguageSync() {
  const locale = useLanguageStore((s) => s.locale);

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  return null;
}

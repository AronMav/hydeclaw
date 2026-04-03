import { create } from "zustand";
import { devtools, persist, subscribeWithSelector } from "zustand/middleware";

export type Locale = "ru" | "en";

export const LOCALES: { value: Locale; label: string }[] = [
  { value: "ru", label: "Русский" },
  { value: "en", label: "English" },
];

interface LanguageState {
  locale: Locale;
  setLocale: (locale: Locale) => void;
}

export const useLanguageStore = create<LanguageState>()(
  devtools(
    subscribeWithSelector(
      persist(
        (set) => ({
          locale: "en",
          setLocale: (locale: Locale) => set({ locale }),
        }),
        { name: "hydeclaw.language" },
      ),
    ),
    { name: "LanguageStore", enabled: process.env.NODE_ENV !== "production" },
  ),
);

import type ru from "./locales/ru.json";

/** All valid translation keys, derived from the Russian (default) locale file */
export type TranslationKey = keyof typeof ru;

/** A complete translations object */
export type Translations = Record<TranslationKey, string>;

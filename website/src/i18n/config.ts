/** The five shipped languages. `en` is the default and carries no URL prefix. */
export const LOCALES = ["en", "tr", "it", "fr", "de"] as const;

export type Locale = (typeof LOCALES)[number];

export const DEFAULT_LOCALE: Locale = "en";

/**
 * Endonyms — each language named in itself. A switcher that lists "Turkish"
 * to somebody who only reads Turkish is a switcher they cannot use.
 */
export const LOCALE_NAMES: Record<Locale, string> = {
  en: "English",
  tr: "Türkçe",
  it: "Italiano",
  fr: "Français",
  de: "Deutsch",
};

/** For `hreflang` and the `lang` attribute. */
export const LOCALE_TAGS: Record<Locale, string> = {
  en: "en",
  tr: "tr",
  it: "it",
  fr: "fr",
  de: "de",
};

export function isLocale(value: string | undefined): value is Locale {
  return !!value && (LOCALES as readonly string[]).includes(value);
}

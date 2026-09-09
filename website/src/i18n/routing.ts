import { LOCALES, DEFAULT_LOCALE, isLocale, type Locale } from "./config";

/**
 * The site is a GitHub Pages *project* site, so everything hangs off
 * `/spacetrace`. Astro exposes that as `import.meta.env.BASE_URL`, and every
 * internal href goes through here rather than being typed by hand: a missing
 * base prefix works in `astro dev` and 404s in production, which is the worst
 * moment to find out.
 */
const BASE = import.meta.env.BASE_URL.replace(/\/+$/, "");

/** Page identifiers, which double as the URL segment. */
export const PAGES = ["", "desktop", "cli", "hub", "download", "guide"] as const;
export type Page = (typeof PAGES)[number];

/** `/spacetrace/tr/desktop/` — locale prefix omitted for the default. */
export function localeUrl(locale: Locale, page: Page = ""): string {
  const prefix = locale === DEFAULT_LOCALE ? "" : `/${locale}`;
  const tail = page ? `/${page}` : "";
  return `${BASE}${prefix}${tail}/`;
}

/** An asset in `public/`, base-prefixed. */
export function asset(path: string): string {
  return `${BASE}/${path.replace(/^\/+/, "")}`;
}

/**
 * Which language is this URL? Derived from the path rather than passed around,
 * so a page cannot render one language's copy under another's URL.
 */
export function localeFromUrl(url: URL): Locale {
  const segment = url.pathname.replace(BASE, "").split("/").filter(Boolean)[0];
  return isLocale(segment) ? segment : DEFAULT_LOCALE;
}

/** Which page is this URL? Used by the language switcher to stay put. */
export function pageFromUrl(url: URL): Page {
  const parts = url.pathname.replace(BASE, "").split("/").filter(Boolean);
  const rest = isLocale(parts[0]) ? parts.slice(1) : parts;
  const candidate = rest[0] ?? "";
  return (PAGES as readonly string[]).includes(candidate)
    ? (candidate as Page)
    : "";
}

/** Every locale's URL for one page, for `hreflang` and the switcher. */
export function alternates(page: Page): { locale: Locale; url: string }[] {
  return LOCALES.map((locale) => ({ locale, url: localeUrl(locale, page) }));
}

/** The non-default locales, for `getStaticPaths`. */
export function translatedLocales(): Locale[] {
  return LOCALES.filter((l) => l !== DEFAULT_LOCALE);
}

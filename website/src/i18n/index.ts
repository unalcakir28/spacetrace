import { DEFAULT_LOCALE, type Locale } from "./config";
import { en, type Dictionary } from "./ui/en";
import { tr } from "./ui/tr";
import { it } from "./ui/it";
import { fr } from "./ui/fr";
import { de } from "./ui/de";

const DICTIONARIES: Record<Locale, Dictionary> = { en, tr, it, fr, de };

/**
 * All five dictionaries are typed against the English one, so a missing key is
 * a build error rather than a blank space on a live page. There is therefore no
 * runtime fallback to write — if it compiles, every string exists.
 */
export function t(locale: Locale): Dictionary {
  return DICTIONARIES[locale] ?? DICTIONARIES[DEFAULT_LOCALE];
}

export type { Dictionary };
export { LOCALES, LOCALE_NAMES, LOCALE_TAGS, DEFAULT_LOCALE, isLocale } from "./config";
export type { Locale };

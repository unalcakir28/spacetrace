/** Byte formatting that matches the application's own — binary units, one
 *  decimal below 10, none above. Locale-aware decimal separator, because a
 *  German reader expects 19,4 GiB and a British one 19.4 GiB. */
const UNITS = ["B", "KiB", "MiB", "GiB", "TiB"] as const;

export function bytes(value: number, locale: string): string {
  let n = value;
  let unit = 0;
  while (n >= 1024 && unit < UNITS.length - 1) {
    n /= 1024;
    unit += 1;
  }
  const digits = unit === 0 ? 0 : n >= 10 ? 1 : 2;
  const formatted = new Intl.NumberFormat(locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(n);
  return `${formatted} ${UNITS[unit]}`;
}

export function percent(fraction: number, locale: string): string {
  return new Intl.NumberFormat(locale, {
    style: "percent",
    maximumFractionDigits: fraction < 0.01 ? 2 : 1,
  }).format(fraction);
}

export function count(value: number, locale: string): string {
  return new Intl.NumberFormat(locale).format(value);
}

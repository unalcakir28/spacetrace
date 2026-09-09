import { CHANNELS, RELEASES_API, type Component } from "../data/releases";

/**
 * Progressive enhancement, and nothing more.
 *
 * Every download link on the site already points at a real file before this
 * runs: the `continuous` tags never move, so the markup alone is a working
 * download page with JavaScript off, on a rate-limited IP, or when
 * api.github.com is having a bad day.
 *
 * What this adds is the truth about what is behind those links — version, date,
 * size — and an upgrade from the rolling build to the newest stable release
 * once one exists. Nothing here may leave the page worse than the markup it
 * started from, so every step is guarded and failure is silent by design.
 */

interface ApiAsset {
  name: string;
  size: number;
  browser_download_url: string;
}

interface ApiRelease {
  tag_name: string;
  draft: boolean;
  prerelease: boolean;
  published_at: string | null;
  created_at: string;
  assets: ApiAsset[];
}

type Os = "macos" | "windows" | "linux" | "other";

function detectOs(): Os {
  const data = (
    navigator as Navigator & {
      userAgentData?: { platform?: string };
    }
  ).userAgentData;
  const hint = `${data?.platform ?? ""} ${navigator.userAgent} ${
    navigator.platform ?? ""
  }`;
  if (/mac|darwin/i.test(hint)) return "macos";
  if (/win/i.test(hint)) return "windows";
  if (/android/i.test(hint)) return "other";
  if (/linux|x11|cros/i.test(hint)) return "linux";
  return "other";
}

function applyOs(): void {
  const os = detectOs();
  document.documentElement.dataset.os = os;
  for (const el of document.querySelectorAll<HTMLElement>("[data-for-os]")) {
    const wanted = (el.dataset.forOs ?? "").split(/\s+/).filter(Boolean);
    el.hidden = !wanted.includes(os);
  }

  // The hero button points at the download page in the markup, which is the
  // answer that is always right and needs no script. When the platform is
  // known it is upgraded in place to the file itself — the label does not
  // change, because an action should keep its name through the whole flow.
  for (const el of document.querySelectorAll<HTMLAnchorElement>("[data-hero-dl]")) {
    const match = el.dataset[`match${os[0].toUpperCase()}${os.slice(1)}`];
    if (!match) continue;
    el.dataset.dl = el.dataset.heroDl || "desktop";
    el.dataset.match = match;
  }
}

function formatBytes(value: number, locale: string): string {
  if (!Number.isFinite(value) || value <= 0) return "";
  const units = ["B", "KiB", "MiB", "GiB"];
  let n = value;
  let unit = 0;
  while (n >= 1024 && unit < units.length - 1) {
    n /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || n >= 10 ? 0 : 1;
  // Non-breaking space, so a size never wraps between figure and unit.
  return `${new Intl.NumberFormat(locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(n)}\u00a0${units[unit]}`;
}

function formatDate(iso: string, locale: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat(locale, {
    day: "numeric",
    month: "short",
    year: "numeric",
  }).format(date);
}

/** The newest stable release for a component, or its rolling build. */
function pick(releases: ApiRelease[], component: Component): ApiRelease | null {
  const spec = CHANNELS[component];
  let stable: ApiRelease | null = null;
  let rolling: ApiRelease | null = null;
  for (const release of releases) {
    if (release.draft) continue;
    if (release.tag_name === spec.rolling) {
      rolling = release;
    } else if (!stable && !release.prerelease && spec.stable.test(release.tag_name)) {
      // The API answers newest first, so the first match is the newest.
      stable = release;
    }
  }
  return stable ?? rolling;
}

function applyRelease(
  component: Component,
  release: ApiRelease,
  locale: string,
  labels: { latestBuild: string; stable: string; notARelease: string },
): void {
  const isStable = !release.prerelease;
  const version = release.tag_name.replace(/^(desktop|hub)-/, "");

  for (const el of document.querySelectorAll<HTMLElement>(
    `[data-version="${component}"]`,
  )) {
    el.textContent = isStable ? version : labels.latestBuild;
  }

  for (const el of document.querySelectorAll<HTMLElement>(
    `[data-published="${component}"]`,
  )) {
    el.textContent = formatDate(release.published_at ?? release.created_at, locale);
  }

  for (const el of document.querySelectorAll<HTMLElement>(
    `[data-channel="${component}"]`,
  )) {
    el.textContent = isStable ? labels.stable : labels.notARelease;
  }

  for (const link of document.querySelectorAll<HTMLAnchorElement>(
    `a[data-dl="${component}"]`,
  )) {
    const needle = link.dataset.match ?? "";
    const asset = release.assets.find((candidate) => candidate.name.includes(needle));
    if (!asset) continue;

    link.href = asset.browser_download_url;

    // In a table the size lives in a sibling cell, so the row is the scope; in
    // a platform card it sits next to the button.
    const scope = link.closest("tr") ?? link.parentElement;
    const size = scope?.querySelector<HTMLElement>("[data-size]");
    if (size) size.textContent = formatBytes(asset.size, locale);

    const filename = link.querySelector<HTMLElement>("[data-filename]");
    if (filename) filename.textContent = asset.name;
  }
}

export function initReleases(
  locale: string,
  labels: { latestBuild: string; stable: string; notARelease: string },
): void {
  applyOs();

  if (!("fetch" in window)) return;
  fetch(RELEASES_API, { headers: { Accept: "application/vnd.github+json" } })
    .then((response) => {
      if (!response.ok) throw new Error(String(response.status));
      return response.json() as Promise<ApiRelease[]>;
    })
    .then((releases) => {
      if (!Array.isArray(releases) || releases.length === 0) return;
      for (const component of ["desktop", "cli", "hub"] as Component[]) {
        const release = pick(releases, component);
        if (release) applyRelease(component, release, locale, labels);
      }
      document.documentElement.dataset.releases = "live";
    })
    .catch(() => {
      /* The markup's own links stand. */
    });
}

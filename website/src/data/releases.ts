/**
 * The download contract.
 *
 * Tag names and asset names here are the same strings the three release
 * workflows produce (see docs/RELEASING.md). Renaming one on either side
 * breaks every download link on the site, so they live in exactly one place
 * and nothing else on the site hard-codes a URL.
 */

export const OWNER = "unalcakir28";
export const REPO = "spacetrace";
export const REPO_SLUG = `${OWNER}/${REPO}`;
export const REPO_URL = `https://github.com/${REPO_SLUG}`;
// Only the public repository is ever linked from the site. The desktop and
// hub sources are private, so a link to them would send a visitor to a sign-in
// page; their downloads are published into the public repo instead, which is
// what the asset URLs below point at.
export const RELEASES_API = `https://api.github.com/repos/${REPO_SLUG}/releases?per_page=40`;
export const INSTALL_SH = `https://raw.githubusercontent.com/${REPO_SLUG}/main/install.sh`;

export type Component = "desktop" | "cli" | "hub";

/** Rolling tag, and the pattern that recognises a stable release. */
export const CHANNELS: Record<Component, { rolling: string; stable: RegExp }> = {
  desktop: { rolling: "desktop-continuous", stable: /^desktop-v\d/ },
  cli: { rolling: "continuous", stable: /^v\d/ },
  hub: { rolling: "hub-continuous", stable: /^hub-v\d/ },
};

export type Os = "macos" | "windows" | "linux";

export interface Asset {
  /** Which release the file comes from. */
  component: Component;
  /** A substring that identifies the file within a release's asset list. */
  match: string;
  /** The file name in the rolling channel — the fallback href. */
  fallbackName: string;
  /** Which platform card this belongs to, if any. */
  os?: Os;
  /** Set on the one file a visitor on that platform should take first. */
  primary?: boolean;
}

function url(component: Component, name: string): string {
  return `${REPO_URL}/releases/download/${CHANNELS[component].rolling}/${name}`;
}

/** The href to use before (or without) the releases API answering. */
export function fallbackUrl(asset: Asset): string {
  return url(asset.component, asset.fallbackName);
}

export function checksumsUrl(component: Component): string {
  return url(component, "SHA256SUMS");
}

export const DESKTOP_ASSETS: Asset[] = [
  {
    component: "desktop",
    match: "macos-universal.dmg",
    fallbackName: "spacetrace-desktop-continuous-macos-universal.dmg",
    os: "macos",
    primary: true,
  },
  {
    component: "desktop",
    match: "windows-x86_64-setup.exe",
    fallbackName: "spacetrace-desktop-continuous-windows-x86_64-setup.exe",
    os: "windows",
    primary: true,
  },
  {
    component: "desktop",
    match: "linux-x86_64.deb",
    fallbackName: "spacetrace-desktop-continuous-linux-x86_64.deb",
    os: "linux",
    primary: true,
  },
  {
    component: "desktop",
    match: "linux-x86_64.rpm",
    fallbackName: "spacetrace-desktop-continuous-linux-x86_64.rpm",
    os: "linux",
  },
  {
    component: "desktop",
    match: "linux-x86_64.AppImage",
    fallbackName: "spacetrace-desktop-continuous-linux-x86_64.AppImage",
    os: "linux",
  },
];

export const CLI_ASSETS: Asset[] = [
  {
    component: "cli",
    match: "aarch64-apple-darwin.tar.gz",
    fallbackName: "spacetrace-continuous-aarch64-apple-darwin.tar.gz",
    os: "macos",
  },
  {
    component: "cli",
    match: "x86_64-apple-darwin.tar.gz",
    fallbackName: "spacetrace-continuous-x86_64-apple-darwin.tar.gz",
    os: "macos",
  },
  {
    component: "cli",
    match: "x86_64-unknown-linux-musl.tar.gz",
    fallbackName: "spacetrace-continuous-x86_64-unknown-linux-musl.tar.gz",
    os: "linux",
  },
  {
    component: "cli",
    match: "aarch64-unknown-linux-musl.tar.gz",
    fallbackName: "spacetrace-continuous-aarch64-unknown-linux-musl.tar.gz",
    os: "linux",
  },
  {
    component: "cli",
    match: "x86_64-pc-windows-msvc.zip",
    fallbackName: "spacetrace-continuous-x86_64-pc-windows-msvc.zip",
    os: "windows",
    primary: true,
  },
];

export const HUB_ASSETS: Asset[] = [
  {
    component: "hub",
    match: "x86_64-unknown-linux-musl.tar.gz",
    fallbackName: "spacetrace-hub-continuous-x86_64-unknown-linux-musl.tar.gz",
    os: "linux",
  },
  {
    component: "hub",
    match: "aarch64-unknown-linux-musl.tar.gz",
    fallbackName: "spacetrace-hub-continuous-aarch64-unknown-linux-musl.tar.gz",
    os: "linux",
  },
  {
    component: "hub",
    match: "aarch64-apple-darwin.tar.gz",
    fallbackName: "spacetrace-hub-continuous-aarch64-apple-darwin.tar.gz",
    os: "macos",
  },
  {
    component: "hub",
    match: "x86_64-apple-darwin.tar.gz",
    fallbackName: "spacetrace-hub-continuous-x86_64-apple-darwin.tar.gz",
    os: "macos",
  },
];

export const AGENT_IMAGE = `ghcr.io/${OWNER}/spacetrace`;
export const HUB_IMAGE = `ghcr.io/${OWNER}/spacetrace-hub`;

/** The one-liner the site puts in front of people more than anything else. */
export const INSTALL_COMMAND = `curl -fsSL ${INSTALL_SH} | sh`;

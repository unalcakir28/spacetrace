/**
 * Behavioural check of the built language redirector.
 *
 * It reads the script out of `dist/` and *runs* it, because the way this
 * feature broke was not a syntax error or a failed build: an Astro templating
 * mistake emitted an IIFE that declared four consts and did nothing. The build
 * was green, the script was present in the HTML, and the feature was dead.
 * Asserting the script exists is therefore worthless — the only check with
 * teeth is to execute it and observe where it sends the reader.
 *
 * Run after `astro build`. Exits non-zero on the first failure.
 */
import { readFileSync } from "node:fs";

const DIST = new URL("../dist/", import.meta.url);

/** The inline redirector from one built page, or null if there is none. */
function extract(page) {
  const html = readFileSync(new URL(page, DIST), "utf8");
  const scripts = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)];
  const found = scripts.find((m) => m[1].includes("spacetrace.auto") && m[1].includes("navigator.language"));
  return found ? found[1] : null;
}

/**
 * Run a script in a stub browser and report what it did. `languages` is the
 * browser's preference list; `stored` is a previous explicit choice; `throws`
 * simulates private mode, where touching storage raises.
 */
function run(source, { languages, language, stored = null, throws = false }) {
  const result = { replaced: null, flag: null };
  const storage = (get) => ({
    getItem: (k) => {
      if (throws) throw new Error("denied");
      return k === "spacetrace.lang" ? stored : null;
    },
    setItem: (k, v) => {
      if (throws) throw new Error("denied");
      if (k === "spacetrace.auto") result.flag = v;
    },
  });
  const window = {
    localStorage: storage(),
    sessionStorage: storage(),
    location: { replace: (url) => (result.replaced = url) },
  };
  // `languages: null` models an old browser exposing only navigator.language.
  const navigator = {
    languages: languages ?? undefined,
    language: language ?? (languages ?? [])[0],
  };
  new Function("window", "navigator", source)(window, navigator);
  return result;
}

const home = extract("index.html");
const download = extract("download/index.html");
const turkish = extract("tr/index.html");
const notFound = extract("404.html");

const cases = [
  // The reader's own language wins.
  ["Turkish browser goes to Turkish", () => run(home, { languages: ["tr-TR", "tr"] }).replaced === "/spacetrace/tr/"],
  ["region-only Italian is matched on the base tag", () => run(home, { languages: ["it-CH"] }).replaced === "/spacetrace/it/"],
  ["German", () => run(home, { languages: ["de-AT", "de"] }).replaced === "/spacetrace/de/"],
  ["French", () => run(home, { languages: ["fr"] }).replaced === "/spacetrace/fr/"],
  ["uppercase tags are normalised", () => run(home, { languages: ["TR-tr"] }).replaced === "/spacetrace/tr/"],

  // A reader of English is left alone (rule 3).
  ["English first wins even with Turkish behind it", () => run(home, { languages: ["en-US", "en", "tr"] }).replaced === null],
  ["English only", () => run(home, { languages: ["en-GB"] }).replaced === null],
  ["Turkish ahead of English still goes to Turkish", () => run(home, { languages: ["tr", "en-US"] }).replaced === "/spacetrace/tr/"],

  // An unsupported language stays on English, which is what x-default says.
  ["Spanish stays on English", () => run(home, { languages: ["es-ES", "es"] }).replaced === null],
  ["empty preference list does nothing", () => run(home, { languages: [""] }).replaced === null],
  ["a browser with no navigator.languages falls back to navigator.language", () => run(home, { languages: null, language: "tr-TR" }).replaced === "/spacetrace/tr/"],

  // An explicit choice wins for good (rule 2).
  ["a stored choice of English is respected", () => run(home, { languages: ["tr-TR"], stored: "en" }).replaced === null],
  ["a stored choice of Turkish stops the redirect too", () => run(home, { languages: ["tr-TR"], stored: "tr" }).replaced === null],

  // Any failure leaves the page alone (rule 4).
  ["storage throwing leaves the page put", () => run(home, { languages: ["tr-TR"], throws: true }).replaced === null],

  // The notice needs the one-shot flag.
  ["a redirect records the flag for the notice", () => run(home, { languages: ["tr-TR"] }).flag === "tr"],
  ["no redirect records no flag", () => run(home, { languages: ["en-US"] }).flag === null],

  // The route is preserved, not flattened to the home page.
  ["the download page keeps its route", () => run(download, { languages: ["tr-TR"] }).replaced === "/spacetrace/tr/download/"],

  // Rule 1: only the unprefixed pages carry the redirector at all.
  ["a translated page has no redirector", () => turkish === null],
  ["404 has no redirector", () => notFound === null],

  // The regression that shipped: a present-but-inert script.
  ["the built script actually contains the redirect", () => home !== null && home.includes("window.location.replace")],
];

let failed = 0;
for (const [name, check] of cases) {
  let ok = false;
  let detail = "";
  try {
    ok = check() === true;
  } catch (error) {
    detail = ` (threw: ${error.message})`;
  }
  if (!ok) failed++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail}`);
}

console.log(`\n${cases.length - failed}/${cases.length} passed`);
if (failed > 0) process.exit(1);

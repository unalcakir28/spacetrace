// @ts-check
import { defineConfig } from "astro/config";
import react from "@astrojs/react";
import sitemap from "@astrojs/sitemap";

// GitHub Pages project site, so the whole thing lives under /spacetrace.
// Every internal link goes through localeUrl() in src/i18n/routing.ts rather
// than being written by hand, because forgetting the base prefix produces
// links that work in dev and 404 in production — the worst kind of bug to
// discover after a deploy.
export default defineConfig({
  site: "https://unalcakir28.github.io",
  base: "/spacetrace",
  trailingSlash: "always",
  integrations: [
    react(),
    sitemap({
      i18n: {
        defaultLocale: "en",
        locales: { en: "en", tr: "tr", it: "it", fr: "fr", de: "de" },
      },
    }),
  ],
  build: {
    // Inline nothing: the interactive islands are small enough that an extra
    // request costs less than blocking the parser.
    inlineStylesheets: "auto",
  },
});

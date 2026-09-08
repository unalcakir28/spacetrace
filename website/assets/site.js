/* Progressive enhancement only.
   ---------------------------------------------------------------------------
   Every download link on this site already points at a real file before this
   script runs: the `continuous` tags never move, so the markup alone is a
   working download page with JavaScript switched off, on a rate-limited IP, or
   when api.github.com is having a bad day.

   What the script adds is the truth about what is behind those links — the
   version, the date, the size — and an upgrade from the rolling build to the
   newest stable release once one exists. Nothing here may leave the page
   worse than the markup it started from, so every step is guarded and a
   failure is silent by design. */

(function () {
  "use strict";

  var REPO = "unalcakir28/spacetrace";

  /* Each component publishes into the same public repository under its own tag
     prefix, so one API call describes all three. */
  var COMPONENTS = {
    cli: { rolling: "continuous", stable: /^v\d/ },
    desktop: { rolling: "desktop-continuous", stable: /^desktop-v\d/ },
    hub: { rolling: "hub-continuous", stable: /^hub-v\d/ },
  };

  /* ------------------------------------------------------------ copying */

  function initCopy() {
    document.querySelectorAll(".cmd").forEach(function (box) {
      var code = box.querySelector("code");
      var button = box.querySelector("button");
      if (!code || !button) return;

      button.addEventListener("click", function () {
        var text = code.textContent.trim();
        var done = function () {
          var was = button.getAttribute("data-label") || button.textContent;
          button.setAttribute("data-label", was);
          button.textContent = "Copied";
          button.setAttribute("data-copied", "yes");
          window.setTimeout(function () {
            button.textContent = was;
            button.removeAttribute("data-copied");
          }, 1600);
        };

        if (navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(text).then(done, function () {});
          return;
        }
        // Older Safari and any non-secure context.
        var ta = document.createElement("textarea");
        ta.value = text;
        ta.setAttribute("readonly", "");
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        try {
          document.execCommand("copy");
          done();
        } catch (e) {
          /* Leave the text on screen; it is selectable. */
        }
        document.body.removeChild(ta);
      });
    });
  }

  /* ---------------------------------------------------- which machine is this */

  function detectOs() {
    var ua = navigator.userAgent;
    var platform = navigator.platform || "";
    // userAgentData is the only way to tell an arm64 Windows or Linux box
    // apart, and it is not everywhere; where it is missing the page offers the
    // x86_64 build and the table right below it has the rest.
    if (/Mac|Darwin/i.test(ua) || /Mac/i.test(platform)) return "macos";
    if (/Win/i.test(ua) || /Win/i.test(platform)) return "windows";
    if (/Android/i.test(ua)) return "other";
    if (/Linux|X11/i.test(ua)) return "linux";
    return "other";
  }

  function initOs() {
    var os = detectOs();
    document.documentElement.setAttribute("data-os", os);

    document.querySelectorAll("[data-for-os]").forEach(function (el) {
      var wanted = el.getAttribute("data-for-os").split(/\s+/);
      var match = wanted.indexOf(os) !== -1;
      el.hidden = !match;
      // The fallback block shows only when nothing else matched.
      if (el.getAttribute("data-for-os") === "other" && os === "other") {
        el.hidden = false;
      }
    });
  }

  /* ------------------------------------------------------------- releases */

  function bytes(n) {
    if (!n || n < 0) return "";
    var units = ["B", "KiB", "MiB", "GiB"];
    var i = 0;
    while (n >= 1024 && i < units.length - 1) {
      n = n / 1024;
      i++;
    }
    return (n >= 10 || i === 0 ? Math.round(n) : n.toFixed(1)) + " " + units[i];
  }

  function when(iso) {
    var d = new Date(iso);
    if (isNaN(d.getTime())) return "";
    // en-GB rather than the visitor's locale: this project has no i18n layer
    // and everything user-facing is English (see the core repo's DECISIONS
    // K1), so a localised month name inside an English sentence reads as a
    // fault rather than a courtesy.
    return d.toLocaleDateString("en-GB", {
      day: "numeric",
      month: "short",
      year: "numeric",
    });
  }

  /** The newest stable release for a component, or its rolling build. */
  function pick(releases, spec) {
    var stable = null;
    var rolling = null;
    releases.forEach(function (r) {
      if (r.draft) return;
      if (r.tag_name === spec.rolling) {
        rolling = r;
      } else if (spec.stable.test(r.tag_name) && !r.prerelease && !stable) {
        // The API returns newest first, so the first match is the newest.
        stable = r;
      }
    });
    return stable || rolling;
  }

  function applyRelease(name, release) {
    var channel = release.prerelease ? "rolling" : "stable";
    var version = release.tag_name.replace(/^(desktop|hub)-/, "");

    document
      .querySelectorAll('[data-version="' + name + '"]')
      .forEach(function (el) {
        el.textContent = channel === "stable" ? version : "latest build";
      });

    document
      .querySelectorAll('[data-published="' + name + '"]')
      .forEach(function (el) {
        el.textContent = when(release.published_at || release.created_at);
      });

    document
      .querySelectorAll('[data-channel="' + name + '"]')
      .forEach(function (el) {
        el.textContent =
          channel === "stable"
            ? "stable release"
            : "built from main, not a release";
      });

    document
      .querySelectorAll('a[data-dl="' + name + '"]')
      .forEach(function (link) {
        var needle = link.getAttribute("data-match") || "";
        var asset = null;
        (release.assets || []).forEach(function (a) {
          if (!asset && a.name.indexOf(needle) !== -1) asset = a;
        });
        if (!asset) return;

        link.href = asset.browser_download_url;
        // In a table the size lives in a sibling cell, so the row is the
        // scope; in the platform card it sits next to the button.
        var scope = link.closest("tr") || link.parentElement;
        var size = scope ? scope.querySelector("[data-size]") : null;
        if (size) size.textContent = bytes(asset.size);
        var label = link.querySelector("[data-filename]");
        if (label) label.textContent = asset.name;
      });
  }

  function initReleases() {
    if (!window.fetch) return;
    fetch("https://api.github.com/repos/" + REPO + "/releases?per_page=40", {
      headers: { Accept: "application/vnd.github+json" },
    })
      .then(function (r) {
        if (!r.ok) throw new Error(String(r.status));
        return r.json();
      })
      .then(function (releases) {
        if (!Array.isArray(releases) || !releases.length) return;
        Object.keys(COMPONENTS).forEach(function (name) {
          var release = pick(releases, COMPONENTS[name]);
          if (release) applyRelease(name, release);
        });
        document.documentElement.setAttribute("data-releases", "live");
      })
      .catch(function () {
        /* The markup's own links stand. */
      });
  }

  /* ------------------------------------------------------ contents highlight */

  function initToc() {
    var toc = document.querySelector("nav.toc");
    if (!toc || !("IntersectionObserver" in window)) return;

    var links = {};
    toc.querySelectorAll('a[href^="#"]').forEach(function (a) {
      links[a.getAttribute("href").slice(1)] = a;
    });

    var targets = Object.keys(links)
      .map(function (id) {
        return document.getElementById(id);
      })
      .filter(Boolean);
    if (!targets.length) return;

    var seen = {};
    var observer = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (e) {
          seen[e.target.id] = e.isIntersecting;
        });
        var current = null;
        targets.forEach(function (t) {
          if (seen[t.id] && !current) current = t.id;
        });
        Object.keys(links).forEach(function (id) {
          links[id].classList.toggle("active", id === current);
        });
      },
      { rootMargin: "-70px 0px -70% 0px" }
    );
    targets.forEach(function (t) {
      observer.observe(t);
    });
  }

  function ready(fn) {
    if (document.readyState !== "loading") fn();
    else document.addEventListener("DOMContentLoaded", fn);
  }

  ready(function () {
    initCopy();
    initOs();
    initToc();
    initReleases();
  });
})();

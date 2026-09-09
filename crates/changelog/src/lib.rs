//! The changelog for all three spacetrace components, in five languages.
//!
//! **Why a data file and not the commit log.** Commits say what was done to
//! the code, in Turkish, for whoever maintains it. A changelog says what
//! changed for whoever uses it. Those are different texts with different
//! audiences, and no tool derives the second from the first: this history is
//! deliberately prose (`APFS clone'larını bir kez say`), so every
//! conventional-commit generator classifies all of it as "other".
//!
//! **Why one file for three components.** Desktop and hub publish their
//! downloads into the core repo's releases already, and the website reads one
//! public place. Two of the three repos are private, so splitting the source
//! across them would mean either a sync job or handing the website a token.
//! The cost is that a desktop change needs an entry committed here; the
//! release procedure in `docs/RELEASING.md` spells out the order.
//!
//! **Why embedded rather than fetched.** The desktop app shows what changed
//! right after it updates itself, which is exactly the moment it may have no
//! network. An offline "what's new" that is blank is worse than none.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

pub mod render;

/// The locales every entry must carry. Same set as the website, deliberately:
/// a reader who picks Italian there and opens the app should not fall back to
/// English for the release notes alone.
pub const LOCALES: [&str; 5] = ["en", "tr", "it", "fr", "de"];

/// The locale every other one falls back to, and the one the generated
/// `CHANGELOG.md` and GitHub release notes are written in (decision K1).
pub const DEFAULT_LOCALE: &str = "en";

const SOURCE: &str = include_str!("../changelog.json");

/// The parsed changelog.
///
/// Panics only if the embedded file is malformed, which cannot reach a release:
/// `validate_source` is a test, so a broken file fails CI before it is built
/// into anything.
pub fn changelog() -> &'static Changelog {
    static PARSED: OnceLock<Changelog> = OnceLock::new();
    PARSED.get_or_init(|| {
        serde_json::from_str(SOURCE).expect("changelog.json is embedded and validated by a test")
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Changelog {
    pub schema: u32,
    pub components: Components,
}

/// Named fields rather than a map: adding a fourth component should be a
/// deliberate edit here, not something a typo in a key can invent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Components {
    pub cli: ComponentLog,
    pub desktop: ComponentLog,
    pub hub: ComponentLog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentLog {
    /// Landed but not yet in a tagged release. The continuous build's notes
    /// are these; cutting a release moves them into `releases`.
    #[serde(default)]
    pub unreleased: Vec<Entry>,
    /// Newest first. Enforced, because both the renderer and the website show
    /// them in file order.
    pub releases: Vec<Release>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    /// ISO 8601 calendar date, `YYYY-MM-DD`.
    pub date: String,
    /// False for a development milestone that was never tagged and has no
    /// downloadable files. The work happened and is worth recording; claiming
    /// a release nobody can install would not be.
    #[serde(default = "yes")]
    pub published: bool,
    pub entries: Vec<Entry>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub kind: Kind,
    /// Locale code to text. Commands, flags, file names and tags stay
    /// untranslated inside these strings - a translated command is false
    /// information.
    pub text: BTreeMap<String, String>,
}

impl Entry {
    /// The text in `locale`, falling back to English.
    ///
    /// The fallback cannot normally fire: [`validate`] requires every locale on
    /// every entry. It exists for the desktop app, which may hold a changelog
    /// built before a locale was added to the app itself.
    pub fn localized(&self, locale: &str) -> &str {
        if let Some(text) = self.text.get(locale) {
            return text;
        }
        self.text
            .get(DEFAULT_LOCALE)
            .map(String::as_str)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    Cli,
    Desktop,
    Hub,
}

impl Component {
    pub const ALL: [Component; 3] = [Component::Cli, Component::Desktop, Component::Hub];

    pub fn slug(self) -> &'static str {
        match self {
            Component::Cli => "cli",
            Component::Desktop => "desktop",
            Component::Hub => "hub",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.slug() == slug)
    }

    /// How this component's releases are titled in generated English output.
    pub fn title(self) -> &'static str {
        match self {
            Component::Cli => "spacetrace CLI and agent",
            Component::Desktop => "spacetrace desktop",
            Component::Hub => "spacetrace hub",
        }
    }

    /// The release tag for a version. Derived rather than stored: the tag names
    /// are a contract the website and `install.sh` bind to, and a field would
    /// be one more place for them to drift.
    pub fn tag(self, version: &str) -> String {
        match self {
            Component::Cli => format!("v{version}"),
            Component::Desktop => format!("desktop-v{version}"),
            Component::Hub => format!("hub-v{version}"),
        }
    }

    /// The rolling tag, which never moves and always holds the tip of `main`.
    pub fn rolling_tag(self) -> &'static str {
        match self {
            Component::Cli => "continuous",
            Component::Desktop => "desktop-continuous",
            Component::Hub => "hub-continuous",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Added,
    Changed,
    Performance,
    Fixed,
    Removed,
    Security,
}

impl Kind {
    /// Rendering order. Keep a Changelog's order, with performance inserted
    /// where it belongs for a tool whose whole job is measuring things.
    pub const ALL: [Kind; 6] = [
        Kind::Added,
        Kind::Changed,
        Kind::Performance,
        Kind::Fixed,
        Kind::Removed,
        Kind::Security,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Kind::Added => "added",
            Kind::Changed => "changed",
            Kind::Performance => "performance",
            Kind::Fixed => "fixed",
            Kind::Removed => "removed",
            Kind::Security => "security",
        }
    }

    /// The English heading, for `CHANGELOG.md` and release notes. The website
    /// and the desktop app translate `slug()` instead.
    pub fn heading(self) -> &'static str {
        match self {
            Kind::Added => "Added",
            Kind::Changed => "Changed",
            Kind::Performance => "Performance",
            Kind::Fixed => "Fixed",
            Kind::Removed => "Removed",
            Kind::Security => "Security",
        }
    }
}

impl Changelog {
    pub fn component(&self, component: Component) -> &ComponentLog {
        match component {
            Component::Cli => &self.components.cli,
            Component::Desktop => &self.components.desktop,
            Component::Hub => &self.components.hub,
        }
    }

    pub fn component_mut(&mut self, component: Component) -> &mut ComponentLog {
        match component {
            Component::Cli => &mut self.components.cli,
            Component::Desktop => &mut self.components.desktop,
            Component::Hub => &mut self.components.hub,
        }
    }

    /// Every problem found, as human-readable lines. Empty means the file is
    /// sound.
    ///
    /// This is the Rust-side counterpart of the website's typed dictionaries:
    /// there a missing locale is a compile error, here a failing test.
    ///
    /// What it guarantees is that every entry carries all five locales and
    /// that none of them is blank or the English pasted in verbatim. It cannot
    /// judge whether a translation is any *good* — nothing mechanical can, and
    /// claiming otherwise would be worse than the gap.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();

        if self.schema != 1 {
            problems.push(format!("unknown schema version {}", self.schema));
        }

        for component in Component::ALL {
            let log = self.component(component);
            let slug = component.slug();

            for (index, entry) in log.unreleased.iter().enumerate() {
                check_entry(entry, &format!("{slug}/unreleased[{index}]"), &mut problems);
            }

            let mut previous: Option<(u64, u64, u64)> = None;
            for release in &log.releases {
                let where_ = format!("{slug} {}", release.version);

                let Some(parsed) = parse_version(&release.version) else {
                    problems.push(format!("{where_}: version is not major.minor.patch"));
                    continue;
                };

                if let Some(previous) = previous {
                    if parsed >= previous {
                        problems.push(format!(
                            "{where_}: releases must be newest first and each version distinct"
                        ));
                    }
                }
                previous = Some(parsed);

                if !is_iso_date(&release.date) {
                    problems.push(format!("{where_}: date is not a YYYY-MM-DD calendar date"));
                }

                if release.entries.is_empty() {
                    problems.push(format!("{where_}: a release with no entries says nothing"));
                }

                for (index, entry) in release.entries.iter().enumerate() {
                    check_entry(entry, &format!("{where_}[{index}]"), &mut problems);
                }
            }
        }

        problems
    }

    /// The newest release of a component, published or not.
    pub fn latest(&self, component: Component) -> Option<&Release> {
        self.component(component).releases.first()
    }

    /// The newest release anyone can actually download.
    pub fn latest_published(&self, component: Component) -> Option<&Release> {
        self.component(component)
            .releases
            .iter()
            .find(|release| release.published)
    }

    pub fn release(&self, component: Component, version: &str) -> Option<&Release> {
        let wanted = version.trim_start_matches('v');
        self.component(component)
            .releases
            .iter()
            .find(|release| release.version == wanted)
    }
}

fn check_entry(entry: &Entry, where_: &str, problems: &mut Vec<String>) {
    for locale in LOCALES {
        let Some(text) = entry.text.get(locale) else {
            problems.push(format!("{where_}: missing {locale}"));
            continue;
        };
        if text.trim().is_empty() {
            problems.push(format!("{where_}: {locale} is empty"));
        }
        if text.trim() != text {
            problems.push(format!("{where_}: {locale} has leading or trailing space"));
        }
    }

    for locale in entry.text.keys() {
        if !LOCALES.contains(&locale.as_str()) {
            problems.push(format!("{where_}: {locale} is not a supported locale"));
        }
    }

    // The failure that actually happens. A missing locale is caught above, but
    // the likelier mistake is pasting the English in as a placeholder to get
    // the entry saved: present, non-empty, trimmed, and untranslated. Short
    // strings are exempt because a real translation can legitimately coincide
    // with the English — a whole sentence cannot.
    const COINCIDENCE_LIMIT: usize = 40;
    let Some(english) = entry.text.get(DEFAULT_LOCALE) else {
        return;
    };
    if english.chars().count() <= COINCIDENCE_LIMIT {
        return;
    }
    for locale in LOCALES {
        if locale == DEFAULT_LOCALE {
            continue;
        }
        if entry.text.get(locale) == Some(english) {
            problems.push(format!("{where_}: {locale} is the English text verbatim"));
        }
    }
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// A real calendar date, not just the shape of one: `2026-02-31` is rejected.
fn is_iso_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }

    let Ok(year) = date[0..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = date[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = date[8..10].parse::<u32>() else {
        return false;
    };

    if !(1..=12).contains(&month) || day == 0 {
        return false;
    }

    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    day <= last
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one test that matters: the file that ships is sound.
    #[test]
    fn validate_source() {
        let problems = changelog().validate();
        assert!(
            problems.is_empty(),
            "changelog.json:\n  {}",
            problems.join("\n  ")
        );
    }

    #[test]
    fn every_component_is_reachable_by_slug() {
        for component in Component::ALL {
            assert_eq!(Component::from_slug(component.slug()), Some(component));
        }
        assert_eq!(Component::from_slug("website"), None);
    }

    #[test]
    fn tags_match_the_published_contract() {
        assert_eq!(Component::Cli.tag("0.2.0"), "v0.2.0");
        assert_eq!(Component::Desktop.tag("0.2.0"), "desktop-v0.2.0");
        assert_eq!(Component::Hub.tag("0.2.0"), "hub-v0.2.0");
    }

    #[test]
    fn a_version_can_be_looked_up_with_or_without_the_v() {
        let log = changelog();
        let plain = log.release(Component::Cli, "0.1.0");
        let tagged = log.release(Component::Cli, "v0.1.0");
        assert!(plain.is_some());
        assert_eq!(plain.map(|r| &r.version), tagged.map(|r| &r.version));
    }

    /// Proves `validate` has teeth, rather than trusting a green run of the
    /// real file: every check below is fed something that should trip it.
    #[test]
    fn validate_rejects_what_it_claims_to_reject() {
        let mut broken = changelog().clone();
        broken.component_mut(Component::Cli).releases[0].entries[0]
            .text
            .remove("de");
        assert!(broken
            .validate()
            .iter()
            .any(|problem| problem.contains("missing de")));

        let mut broken = changelog().clone();
        broken.component_mut(Component::Cli).releases[0].date = "2026-02-31".into();
        assert!(broken
            .validate()
            .iter()
            .any(|problem| problem.contains("calendar date")));

        let mut broken = changelog().clone();
        broken.component_mut(Component::Cli).releases.reverse();
        assert!(broken
            .validate()
            .iter()
            .any(|problem| problem.contains("newest first")));

        let mut broken = changelog().clone();
        broken.component_mut(Component::Hub).releases[0]
            .entries
            .clear();
        assert!(broken
            .validate()
            .iter()
            .any(|problem| problem.contains("no entries")));

        // A locale filled in with the English text: the placeholder that gets
        // an entry saved and then quietly shipped.
        let mut broken = changelog().clone();
        let entry = &mut broken.component_mut(Component::Cli).releases[0].entries[0];
        let english = entry.text[DEFAULT_LOCALE].clone();
        entry.text.insert("de".to_string(), english);
        assert!(broken
            .validate()
            .iter()
            .any(|problem| problem.contains("de is the English text verbatim")));
    }

    /// A short entry can honestly read the same in two languages, so the
    /// verbatim check must not fire on one.
    #[test]
    fn a_short_entry_may_match_english_without_being_a_placeholder() {
        let short = "ncdu export".to_string();
        let entry = Entry {
            kind: Kind::Added,
            text: LOCALES
                .iter()
                .map(|locale| (locale.to_string(), short.clone()))
                .collect(),
        };
        let mut problems = Vec::new();
        check_entry(&entry, "test", &mut problems);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn dates_are_checked_against_the_calendar() {
        assert!(is_iso_date("2026-09-09"));
        assert!(is_iso_date("2024-02-29"));
        assert!(!is_iso_date("2026-02-29"));
        assert!(!is_iso_date("2026-13-01"));
        assert!(!is_iso_date("2026-09-00"));
        assert!(!is_iso_date("2026-9-9"));
        assert!(!is_iso_date("not a date"));
    }

    #[test]
    fn missing_locales_fall_back_to_english_rather_than_to_nothing() {
        let entry = Entry {
            kind: Kind::Added,
            text: BTreeMap::from([("en".to_string(), "only English".to_string())]),
        };
        assert_eq!(entry.localized("de"), "only English");
        assert_eq!(entry.localized("en"), "only English");
    }
}

//! Turning the changelog into the shapes other things consume.
//!
//! Everything here writes English (decision K1): `CHANGELOG.md` and the GitHub
//! release notes are the pipeline's own output, and the tool is English. The
//! other four locales exist for the website and the desktop app, which read
//! the data directly rather than this text.

use crate::{Changelog, Component, Entry, Kind, Release, DEFAULT_LOCALE};

/// The header of every generated `CHANGELOG.md`.
///
/// It names the component, because two of the three files live in repos where
/// the generator is not a command you can run — the instruction has to say
/// which repo to run it in and with what, or it is just noise above a file
/// somebody will edit by hand anyway.
fn generated_by(component: Component) -> String {
    format!(
        "<!-- Generated from crates/changelog/changelog.json in unalcakir28/spacetrace.\n     \
         Do not edit by hand. From a checkout of that repo:\n       \
         cargo run -p spacetrace-changelog -- markdown --component {} > CHANGELOG.md -->",
        component.slug()
    )
}

/// The whole `CHANGELOG.md` for one component.
pub fn markdown(changelog: &Changelog, component: Component) -> String {
    let log = changelog.component(component);
    let mut out = String::new();

    out.push_str(&generated_by(component));
    out.push_str("\n\n# Changelog\n\n");
    out.push_str(&format!(
        "What changed in {}, newest first.\n",
        component.title()
    ));

    // Only explain the marker when there is one to explain; a note about a
    // situation the reader cannot see is noise.
    if log.releases.iter().any(|release| !release.published) {
        out.push_str(
            "\nVersions marked *development milestone* were never tagged and have no\n\
             downloadable files. They are recorded because the work happened, not\n\
             because anyone can install them.\n",
        );
    }

    if !log.unreleased.is_empty() {
        out.push_str("\n## Unreleased\n");
        out.push_str(&entries(&log.unreleased));
    }

    for release in &log.releases {
        out.push_str(&format!("\n## {}\n", heading(release)));
        out.push_str(&entries(&release.entries));
    }

    out
}

/// The body of a GitHub release, for `gh release create --notes-file`.
///
/// Only the changes: each workflow appends its own platform table and install
/// instructions, which are about the files rather than about the version.
pub fn notes(changelog: &Changelog, component: Component, version: &str) -> Option<String> {
    let release = changelog.release(component, version)?;
    Some(entries(&release.entries).trim_start().to_string())
}

/// The body for a continuous build: what has landed since the last release.
pub fn unreleased_notes(changelog: &Changelog, component: Component) -> String {
    let log = changelog.component(component);
    if log.unreleased.is_empty() {
        return "No changes recorded since the last release.\n".to_string();
    }
    entries(&log.unreleased).trim_start().to_string()
}

fn heading(release: &Release) -> String {
    let Release {
        version,
        date,
        published,
        ..
    } = release;
    if *published {
        return format!("{version} — {date}");
    }
    format!("{version} — {date} · *development milestone*")
}

/// Entries grouped by kind, in [`Kind::ALL`] order, keeping source order within
/// each group.
fn entries(entries: &[Entry]) -> String {
    let mut out = String::new();

    for kind in Kind::ALL {
        let mut group = entries.iter().filter(|entry| entry.kind == kind).peekable();
        if group.peek().is_none() {
            continue;
        }

        out.push_str(&format!("\n### {}\n\n", kind.heading()));
        for entry in group {
            out.push_str(&format!("- {}\n", entry.localized(DEFAULT_LOCALE)));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changelog;

    #[test]
    fn a_component_renders_every_one_of_its_releases() {
        let log = changelog();
        for component in Component::ALL {
            let text = markdown(log, component);
            for release in &log.component(component).releases {
                assert!(
                    text.contains(&format!("## {}", release.version)),
                    "{} is missing {} from its CHANGELOG",
                    component.slug(),
                    release.version
                );
            }
        }
    }

    #[test]
    fn unpublished_releases_say_so_and_published_ones_stay_quiet() {
        let text = markdown(changelog(), Component::Cli);
        assert!(text.contains("development milestone"));

        let published = Release {
            version: "9.9.9".into(),
            date: "2026-01-01".into(),
            published: true,
            entries: Vec::new(),
        };
        assert_eq!(heading(&published), "9.9.9 — 2026-01-01");
    }

    /// Built here rather than read from the real changelog: what is under test
    /// is the grouping, and searching the whole document for two headings finds
    /// whichever release happens to contain them today. That is how this test
    /// broke the first time a release was cut.
    #[test]
    fn kinds_are_grouped_in_a_fixed_order_not_in_source_order() {
        let say = |kind: Kind, text: &str| Entry {
            kind,
            text: std::collections::BTreeMap::from([(
                DEFAULT_LOCALE.to_string(),
                text.to_string(),
            )]),
        };
        // Deliberately the wrong way round in the source.
        let text = entries(&[
            say(Kind::Fixed, "a bug"),
            say(Kind::Added, "a feature"),
            say(Kind::Changed, "a behaviour"),
        ]);

        let at = |heading: &str| text.find(heading).unwrap_or_else(|| panic!("no {heading}"));
        assert!(at("### Added") < at("### Changed"));
        assert!(at("### Changed") < at("### Fixed"));
        // And a kind with nothing in it leaves no empty heading behind.
        assert!(!text.contains("### Security"));
    }

    #[test]
    fn notes_are_the_changes_alone_with_no_leading_blank_line() {
        let text = notes(changelog(), Component::Cli, "0.1.0").expect("0.1.0 exists");
        assert!(
            text.starts_with("### "),
            "release notes began with {text:?}"
        );
        assert!(!text.contains("# Changelog"));
    }

    #[test]
    fn asking_for_a_version_that_does_not_exist_returns_nothing() {
        assert!(notes(changelog(), Component::Cli, "99.0.0").is_none());
    }

    #[test]
    fn a_component_with_nothing_pending_says_so_rather_than_rendering_blank() {
        let mut empty = changelog().clone();
        empty.component_mut(Component::Cli).unreleased.clear();
        assert!(unreleased_notes(&empty, Component::Cli).contains("No changes recorded"));
    }
}

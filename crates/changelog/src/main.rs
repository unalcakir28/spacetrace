//! The changelog generator: a repo tool, not something a release ships.
//!
//! Arguments are parsed by hand rather than with clap. The library half of this
//! crate is a dependency of the desktop app and the hub, and a package's
//! dependencies apply to all of its targets — pulling clap in for six internal
//! subcommands would make both of them compile an argument parser they never
//! run.

use std::path::Path;
use std::process::ExitCode;

use spacetrace_changelog::{changelog, render, Changelog, Component, Release};
#[cfg(test)]
use spacetrace_changelog::{ComponentLog, Components, Entry, Kind, LOCALES};

const SOURCE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/changelog.json");

const USAGE: &str = "\
Render and maintain crates/changelog/changelog.json.

    check                                  validate the source
    fmt                                    rewrite it in canonical form
    json                                   print it in canonical form
    markdown   --component <c>             the whole CHANGELOG.md
    notes      --component <c> --version <v>   one release's notes
    unreleased --component <c>             what has landed since the last release
    promote    --component <c> --version <v> [--date YYYY-MM-DD]
                                           move unreleased into a new release

<c> is cli, desktop or hub.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };

    match run(command, &args[1..]) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: &str, rest: &[String]) -> Result<String, String> {
    match command {
        "check" => check(),
        "fmt" => fmt(),
        "json" => Ok(canonical_json(changelog())?),
        "markdown" => Ok(render::markdown(changelog(), component(rest)?)),
        "unreleased" => Ok(render::unreleased_notes(changelog(), component(rest)?)),
        "notes" => notes(rest),
        "promote" => promote(rest),
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    }
}

fn check() -> Result<String, String> {
    let problems = changelog().validate();
    if problems.is_empty() {
        return Ok(String::new());
    }
    Err(format!("changelog.json:\n  {}", problems.join("\n  ")))
}

fn fmt() -> Result<String, String> {
    let canonical = canonical_json(changelog())?;
    write(Path::new(SOURCE_PATH), &canonical)?;
    Ok(String::new())
}

fn notes(args: &[String]) -> Result<String, String> {
    let component = component(args)?;
    let version = required(args, "--version")?;
    render::notes(changelog(), component, &version).ok_or_else(|| {
        format!(
            "{} has no release {version} in the changelog",
            component.slug()
        )
    })
}

fn promote(args: &[String]) -> Result<String, String> {
    let component = component(args)?;
    let version = required(args, "--version")?;
    let version = version.trim_start_matches('v');
    let date = match optional(args, "--date") {
        Some(date) => date,
        None => today_utc()?,
    };

    let (updated, count) = promoted(changelog(), component, version, &date)?;
    write(Path::new(SOURCE_PATH), &canonical_json(&updated)?)?;

    Ok(format!(
        "{} {version} on {date}: {count} entries promoted. Tag it {}.\n",
        component.slug(),
        component.tag(version),
    ))
}

/// The changelog with a component's `unreleased` entries moved into a new
/// release at the top, and how many moved.
///
/// Separated from the command so it can be tested: this is the one operation
/// that rewrites the single source of the changelog, and a function that can
/// only be exercised by overwriting the real file is a function nobody tests.
///
/// Refuses an empty `unreleased`, because a release whose notes say nothing is
/// worse than one that was never cut — the reader cannot tell whether the notes
/// are missing or the release was empty.
fn promoted(
    changelog: &Changelog,
    component: Component,
    version: &str,
    date: &str,
) -> Result<(Changelog, usize), String> {
    let mut updated = changelog.clone();
    let log = updated.component_mut(component);

    if log.unreleased.is_empty() {
        return Err(format!(
            "{} has nothing unreleased to promote",
            component.slug()
        ));
    }
    if let Some(existing) = log.releases.iter().find(|r| r.version == version) {
        return Err(format!(
            "{} {version} is already released, on {}",
            component.slug(),
            existing.date
        ));
    }

    let entries = std::mem::take(&mut log.unreleased);
    let count = entries.len();
    log.releases.insert(
        0,
        Release {
            version: version.to_string(),
            date: date.to_string(),
            published: true,
            entries,
        },
    );

    // Validate before returning, not after writing: promoting out of order is
    // exactly what this can get wrong, and a refused promotion must leave the
    // file as it was.
    let problems = updated.validate();
    if !problems.is_empty() {
        return Err(format!(
            "promoting would break the changelog:\n  {}",
            problems.join("\n  ")
        ));
    }

    Ok((updated, count))
}

fn canonical_json(changelog: &Changelog) -> Result<String, String> {
    let mut json = serde_json::to_string_pretty(changelog)
        .map_err(|err| format!("cannot serialise the changelog: {err}"))?;
    json.push('\n');
    Ok(json)
}

/// Write by rename, so an interrupted run cannot leave the changelog truncated.
/// It is the source every other artefact is generated from; half of it is worse
/// than the previous version of it.
fn write(path: &Path, contents: &str) -> Result<(), String> {
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, contents)
        .map_err(|err| format!("cannot write {}: {err}", staged.display()))?;
    std::fs::rename(&staged, path)
        .map_err(|err| format!("cannot replace {}: {err}", path.display()))
}

fn component(args: &[String]) -> Result<Component, String> {
    let slug = required(args, "--component")?;
    Component::from_slug(&slug).ok_or_else(|| format!("unknown component {slug}"))
}

fn required(args: &[String], flag: &str) -> Result<String, String> {
    optional(args, flag).ok_or_else(|| format!("{flag} is required\n\n{USAGE}"))
}

fn optional(args: &[String], flag: &str) -> Option<String> {
    let at = args.iter().position(|arg| arg == flag)?;
    args.get(at + 1).cloned()
}

/// Today, UTC, as `YYYY-MM-DD`.
///
/// Hand-written rather than pulling in a date crate, for the same reason the
/// agent's cron parser is: the only thing needed is a calendar date from a Unix
/// timestamp, and that is Hinnant's `civil_from_days` — a dozen lines with a
/// known-correct answer for every day, which the tests check at the awkward
/// ones.
fn today_utc() -> Result<String, String> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "the system clock is before 1970".to_string())?
        .as_secs();
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_and_the_awkward_days_convert_correctly() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29)); // a leap day
        assert_eq!(civil_from_days(11_016), (2000, 2, 29)); // a century leap year
        assert_eq!(civil_from_days(20_705), (2026, 9, 9));
        assert_eq!(civil_from_days(19_783), (2024, 3, 1)); // the day after one
    }

    #[test]
    fn todays_date_is_a_date_the_changelog_would_accept() {
        let today = today_utc().expect("a working clock");
        assert_eq!(today.len(), 10);
        let mut updated = changelog().clone();
        updated.component_mut(Component::Cli).releases[0].date = today;
        assert!(updated
            .validate()
            .iter()
            .all(|problem| !problem.contains("calendar date")));
    }

    #[test]
    fn flags_are_read_by_name_not_by_position() {
        let args: Vec<String> = ["--version", "1.2.3", "--component", "hub"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(component(&args).unwrap(), Component::Hub);
        assert_eq!(required(&args, "--version").unwrap(), "1.2.3");
        assert!(optional(&args, "--date").is_none());
    }

    #[test]
    fn a_flag_with_nothing_after_it_is_missing_rather_than_empty() {
        let args = vec!["--component".to_string()];
        assert!(component(&args).is_err());
    }

    #[test]
    fn an_unknown_component_is_refused_by_name() {
        let args: Vec<String> = ["--component", "website"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(component(&args).unwrap_err().contains("website"));
    }

    /// The round trip must be lossless, or `promote` would quietly rewrite
    /// entries it was not asked to touch.
    #[test]
    fn canonical_json_reparses_to_the_same_changelog() {
        let json = canonical_json(changelog()).expect("serialisable");
        let reparsed: Changelog = serde_json::from_str(&json).expect("reparsable");
        assert_eq!(
            canonical_json(&reparsed).expect("serialisable"),
            json,
            "a round trip through JSON changed the changelog"
        );
    }

    /// A changelog of its own, not the real one.
    ///
    /// These tests used to promote the live `changelog.json`, which meant they
    /// failed the moment a release was actually cut — the fixture stopped
    /// having anything pending. What is under test is the promotion logic, not
    /// today's content.
    fn fixture() -> Changelog {
        let entry = |kind: Kind, text: &str| Entry {
            kind,
            text: LOCALES
                .iter()
                .map(|locale| (locale.to_string(), format!("{locale}: {text}")))
                .collect(),
        };
        let log = || ComponentLog {
            unreleased: vec![
                entry(Kind::Added, "something new"),
                entry(Kind::Fixed, "a bug"),
            ],
            releases: vec![Release {
                version: "1.0.0".into(),
                date: "2026-01-01".into(),
                published: true,
                entries: vec![entry(Kind::Added, "the first one")],
            }],
        };
        Changelog {
            schema: 1,
            components: Components {
                cli: log(),
                desktop: log(),
                hub: log(),
            },
        }
    }

    #[test]
    fn promoting_moves_every_unreleased_entry_into_the_new_release() {
        let before = fixture();
        let pending = before.component(Component::Cli).unreleased.clone();

        let (after, count) =
            promoted(&before, Component::Cli, "1.1.0", "2026-09-10").expect("a clean promotion");
        let log = after.component(Component::Cli);

        assert_eq!(count, pending.len());
        assert!(log.unreleased.is_empty(), "unreleased must be emptied");

        let cut = &log.releases[0];
        assert_eq!(cut.version, "1.1.0");
        assert_eq!(cut.date, "2026-09-10");
        assert!(cut.published, "a promoted release is a real one");
        assert_eq!(cut.entries.len(), pending.len());
        assert_eq!(
            cut.entries[0].localized("tr"),
            pending[0].localized("tr"),
            "entries must move across intact, not be rebuilt"
        );

        assert!(after.validate().is_empty());
        // Everything already released must still be there, untouched.
        assert_eq!(log.releases.len(), 2);
        assert_eq!(log.releases[1].version, "1.0.0");
        // And the other components must not have moved.
        assert_eq!(after.component(Component::Hub).unreleased.len(), 2);
    }

    #[test]
    fn promoting_nothing_is_refused_rather_than_cutting_an_empty_release() {
        let mut empty = fixture();
        empty.component_mut(Component::Hub).unreleased.clear();
        let refusal = promoted(&empty, Component::Hub, "1.1.0", "2026-09-10").unwrap_err();
        assert!(refusal.contains("nothing unreleased"), "{refusal}");
    }

    #[test]
    fn promoting_over_an_existing_version_is_refused() {
        let refusal = promoted(&fixture(), Component::Cli, "1.0.0", "2026-09-10").unwrap_err();
        assert!(refusal.contains("already released"), "{refusal}");
        assert!(refusal.contains("2026-01-01"), "say when: {refusal}");
    }

    /// The ordering rule is the one thing promoting can break, so it must be
    /// caught before the file is written rather than by the next test run.
    #[test]
    fn promoting_a_version_older_than_the_newest_is_refused() {
        let refusal = promoted(&fixture(), Component::Cli, "0.9.0", "2026-09-10").unwrap_err();
        assert!(refusal.contains("newest first"), "{refusal}");
    }

    #[test]
    fn a_refused_promotion_changes_nothing() {
        let before = fixture();
        let untouched = before.clone();
        assert!(promoted(&before, Component::Cli, "0.9.0", "2026-09-10").is_err());
        assert_eq!(
            canonical_json(&untouched).unwrap(),
            canonical_json(&before).unwrap()
        );
    }

    #[test]
    fn the_staging_file_sits_beside_the_target_so_the_rename_stays_atomic() {
        let target = Path::new("/tmp/spacetrace/changelog.json");
        assert_eq!(
            target.with_extension("json.tmp"),
            Path::new("/tmp/spacetrace/changelog.json.tmp"),
            "a rename across filesystems is not atomic"
        );
    }
}

//! How old the bytes are.
//!
//! "400 GB nobody has touched in two years" is a different question from
//! "what is big", and it is usually the more actionable one: the big folder
//! is often the one in daily use, and the one worth moving is the one nobody
//! has opened since the last laptop.
//!
//! Weighted by **bytes, not by file count**. A hundred thousand stale source
//! files are not the answer; one stale disk image is. Both measures are
//! carried for the same reason they are carried everywhere else (invariant
//! #6): the number shown and the thing it is sorted by have to agree.
//!
//! **Directories are not counted.** A directory's mtime moves when an entry
//! is added or removed beside it, which says nothing about the age of what
//! is inside; charging its blocks to a bucket on that basis would make a
//! folder of ancient files look fresh because something was deleted from it
//! last week.

use crate::{EntryKind, Tree};

/// Seconds in a day, for turning an age in seconds into the days the buckets
/// are described in.
const DAY: i64 = 86_400;

/// One age band and what sits in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct AgeBucket {
    /// Upper bound in days, or `None` for the open-ended oldest band.
    pub up_to_days: Option<u32>,
    pub files: u64,
    pub size: u64,
    pub alloc: u64,
}

/// The whole distribution.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AgeProfile {
    /// One per edge given, plus a final open-ended band, oldest last.
    pub buckets: Vec<AgeBucket>,
    /// Files whose modification time was never recorded.
    ///
    /// Its own band rather than the oldest one. A snapshot imported from a
    /// format that does not carry mtime arrives with zero there, and reading
    /// that as 1970 would announce fifty-six-year-old files that are nothing
    /// of the kind — a confident wrong answer, which this project treats as
    /// worse than no answer.
    pub unknown: AgeBucket,
}

/// The default bands, in days: a week, a month, a quarter, a year, two years,
/// and then everything older.
pub const DEFAULT_EDGES: &[u32] = &[7, 30, 90, 365, 730];

impl AgeProfile {
    /// Bytes on disk in every band that lies **entirely** beyond `days`.
    ///
    /// The headline number: "this much has not been touched since…". The
    /// profile is bucketed, so this can only answer honestly at a band
    /// boundary: asking about a day inside a band leaves that whole band out
    /// rather than guessing how much of it qualifies, which means the answer
    /// is a floor and never an overstatement. A number that might be too big
    /// is the one a reader would act on wrongly.
    ///
    /// Files with no recorded time are left out, because including them
    /// would be a claim about when they were last written.
    pub fn alloc_older_than(&self, days: u32) -> u64 {
        self.buckets
            .iter()
            .filter(|b| b.up_to_days.is_none_or(|up_to| up_to > days))
            .map(|b| b.alloc)
            .sum()
    }
}

/// Distribution of a tree's file bytes by age.
///
/// `now` is passed in rather than read from the clock: a function that reads
/// the time cannot be tested against a fixture, and this one is all about
/// boundaries.
///
/// `edges` are upper bounds in days, ascending. Anything beyond the last edge
/// lands in a final open-ended band, so the bands always cover everything.
pub fn age_profile(tree: &Tree, now: i64, edges: &[u32]) -> AgeProfile {
    let mut sorted: Vec<u32> = edges.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut profile = AgeProfile {
        buckets: sorted
            .iter()
            .map(|&d| AgeBucket {
                up_to_days: Some(d),
                ..AgeBucket::default()
            })
            .chain(std::iter::once(AgeBucket::default()))
            .collect(),
        unknown: AgeBucket::default(),
    };

    for id in tree.iter() {
        let node = tree.node(id);
        if node.kind == EntryKind::Dir {
            continue;
        }
        let bucket = match age_days(node.mtime, now) {
            None => &mut profile.unknown,
            Some(days) => {
                let index = sorted
                    .iter()
                    .position(|&edge| days <= i64::from(edge))
                    .unwrap_or(sorted.len());
                &mut profile.buckets[index]
            }
        };
        bucket.files += 1;
        bucket.size += node.own_size;
        bucket.alloc += node.own_alloc;
    }

    profile
}

/// Age in whole days, or `None` when the time was never recorded.
///
/// A timestamp in the future is not an error and not a lie to correct: it
/// happens with clock skew and with archives unpacked from machines set
/// wrong. Its age comes out negative and lands in the newest band, which is
/// the closest true thing.
///
/// The negative is deliberately *not* clamped. A clamp here changes nothing —
/// a negative age falls into the first band exactly as zero does — and a
/// guard that cannot alter an outcome is one nobody can test and everybody
/// later trusts.
fn age_days(mtime: i64, now: i64) -> Option<i64> {
    if mtime <= 0 {
        return None;
    }
    Some((now - mtime) / DAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImportedNode;
    use std::path::PathBuf;

    /// Roughly 2024. Big enough that a several-thousand-day-old file still
    /// has a positive timestamp — with a small `NOW` they come out negative
    /// and are classified as unrecorded, which is right for the code and
    /// wrong for the fixture.
    const NOW: i64 = 20_000 * DAY;

    fn file_aged(name: &str, days: i64, size: u64) -> ImportedNode {
        let mut node = ImportedNode::file(name, size, size);
        node.mtime = NOW - days * DAY;
        node
    }

    fn tree_of(children: Vec<ImportedNode>) -> Tree {
        let mut root = ImportedNode::dir("root");
        root.children = children;
        Tree::from_nested(PathBuf::from("/root"), root)
    }

    fn profile_of(children: Vec<ImportedNode>) -> AgeProfile {
        age_profile(&tree_of(children), NOW, DEFAULT_EDGES)
    }

    #[test]
    fn files_land_in_the_band_their_age_belongs_to() {
        let profile = profile_of(vec![
            file_aged("today", 0, 10),
            file_aged("a-fortnight", 14, 20),
            file_aged("half-a-year", 180, 40),
            file_aged("ancient", 3_000, 80),
        ]);

        let by_edge = |d: Option<u32>| {
            profile
                .buckets
                .iter()
                .find(|b| b.up_to_days == d)
                .copied()
                .unwrap()
        };
        assert_eq!(by_edge(Some(7)).size, 10);
        assert_eq!(by_edge(Some(30)).size, 20);
        assert_eq!(by_edge(Some(365)).size, 40);
        assert_eq!(by_edge(None).size, 80, "beyond the last edge");
    }

    /// The bands are inclusive of their upper bound, and the test says so at
    /// the exact day rather than near it — an off-by-one here moves bytes
    /// between "this month" and "this quarter" and nobody would notice.
    #[test]
    fn a_file_exactly_on_a_boundary_belongs_to_the_lower_band() {
        let profile = profile_of(vec![file_aged("on-the-line", 30, 100)]);
        let thirty = profile
            .buckets
            .iter()
            .find(|b| b.up_to_days == Some(30))
            .unwrap();
        assert_eq!(thirty.size, 100);
        let ninety = profile
            .buckets
            .iter()
            .find(|b| b.up_to_days == Some(90))
            .unwrap();
        assert_eq!(ninety.size, 0);
    }

    /// The whole point of the feature, as a number.
    #[test]
    fn the_headline_is_the_bytes_nobody_has_touched() {
        let profile = profile_of(vec![
            file_aged("fresh", 1, 1_000),
            file_aged("stale", 800, 400_000),
            file_aged("older-still", 2_000, 600_000),
        ]);
        assert_eq!(profile.alloc_older_than(730), 1_000_000);
        assert_eq!(profile.alloc_older_than(365), 1_000_000);
        assert_eq!(profile.alloc_older_than(0), 1_001_000, "everything");
    }

    /// The band whose upper bound *is* the number asked about holds files
    /// younger than it too, so it must not be counted. Without a file inside
    /// that band the question cannot be answered wrongly, which is why this
    /// test exists separately: a mutation loosening the comparison survived
    /// the one above.
    #[test]
    fn a_band_that_only_partly_qualifies_is_left_out() {
        let profile = profile_of(vec![
            file_aged("just-inside-two-years", 400, 7_000),
            file_aged("well-past", 900, 3_000),
        ]);
        assert_eq!(
            profile.alloc_older_than(730),
            3_000,
            "the 400-day file is in the 366..=730 band and is not 730 days old"
        );
        assert_eq!(
            profile.alloc_older_than(365),
            10_000,
            "at the band's own edge both qualify"
        );
    }

    /// A snapshot imported from a format that does not record mtime must not
    /// be reported as half a century old.
    #[test]
    fn a_file_with_no_recorded_time_is_unknown_not_ancient() {
        let mut no_time = ImportedNode::file("mystery", 500, 500);
        no_time.mtime = 0;
        let profile = profile_of(vec![no_time, file_aged("known", 2_000, 10)]);

        assert_eq!(profile.unknown.files, 1);
        assert_eq!(profile.unknown.size, 500);
        assert_eq!(
            profile.buckets.last().unwrap().size,
            10,
            "the unknown file must not be in the oldest band"
        );
        assert_eq!(
            profile.alloc_older_than(730),
            10,
            "and must not be claimed as untouched"
        );
    }

    /// Clock skew and archives from machines set wrong produce these, and
    /// they are not worth failing over.
    #[test]
    fn a_timestamp_in_the_future_counts_as_brand_new() {
        let profile = profile_of(vec![file_aged("tomorrow", -1, 42)]);
        assert_eq!(profile.buckets[0].size, 42);
        assert_eq!(profile.unknown.files, 0);
    }

    /// Directories carry blocks, but their mtime is about their entries
    /// changing, not about the age of what they hold.
    #[test]
    fn directories_are_left_out() {
        let mut old_dir = ImportedNode::dir("archive");
        old_dir.mtime = NOW - 3_000 * DAY;
        old_dir.alloc = 4_096;
        old_dir.children = vec![file_aged("inside", 3_000, 100)];

        let profile = age_profile(&tree_of(vec![old_dir]), NOW, DEFAULT_EDGES);
        let total: u64 = profile.buckets.iter().map(|b| b.alloc).sum();
        assert_eq!(total, 100, "the directory's own 4096 is not in any band");
    }

    /// Every byte of every file is in exactly one band. A file counted twice,
    /// or dropped between two edges, is the failure this catches.
    #[test]
    fn the_bands_account_for_every_file_exactly_once() {
        let files: Vec<ImportedNode> = (0..50)
            .map(|i| file_aged(&format!("f{i}"), i * 37, 1_000 + i as u64))
            .collect();
        let expected_size: u64 = files.iter().map(|f| f.size).sum();
        let expected_files = files.len() as u64;

        let profile = profile_of(files);
        let counted: u64 =
            profile.buckets.iter().map(|b| b.files).sum::<u64>() + profile.unknown.files;
        let summed: u64 =
            profile.buckets.iter().map(|b| b.size).sum::<u64>() + profile.unknown.size;
        assert_eq!(counted, expected_files);
        assert_eq!(summed, expected_size);
    }

    #[test]
    fn edges_given_out_of_order_or_twice_still_produce_clean_bands() {
        let tree = tree_of(vec![file_aged("a", 45, 10)]);
        let profile = age_profile(&tree, NOW, &[90, 7, 30, 30]);
        let edges: Vec<Option<u32>> = profile.buckets.iter().map(|b| b.up_to_days).collect();
        assert_eq!(edges, vec![Some(7), Some(30), Some(90), None]);
        assert_eq!(profile.buckets[2].size, 10);
    }

    #[test]
    fn no_edges_at_all_is_one_band_holding_everything() {
        let tree = tree_of(vec![file_aged("a", 45, 10), file_aged("b", 4_000, 20)]);
        let profile = age_profile(&tree, NOW, &[]);
        assert_eq!(profile.buckets.len(), 1);
        assert_eq!(profile.buckets[0].size, 30);
    }
}

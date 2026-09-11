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

use crate::{EntryKind, NodeId, Tree};

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
    age_profile_at(tree, tree.root(), now, edges)
}

/// The same distribution, for one subtree.
///
/// The desktop's legend has to describe the folder on screen rather than the
/// whole scan: a key whose numbers belong to a different view is worse than no
/// key, because it looks like it agrees.
pub fn age_profile_at(tree: &Tree, root: NodeId, now: i64, edges: &[u32]) -> AgeProfile {
    let sorted = clean_edges(edges);

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

    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        stack.extend(tree.children(id));
        let node = tree.node(id);
        if node.kind == EntryKind::Dir {
            continue;
        }
        let bucket = match band_of(node.mtime, now, &sorted) {
            None => &mut profile.unknown,
            Some(band) => &mut profile.buckets[band],
        };
        bucket.files += 1;
        bucket.size += node.own_size;
        bucket.alloc += node.own_alloc;
    }

    profile
}

/// Edges as the bands actually use them: ascending and without repeats, so a
/// caller's order or a duplicated bound cannot produce a band that no file can
/// ever land in.
fn clean_edges(edges: &[u32]) -> Vec<u32> {
    let mut sorted: Vec<u32> = edges.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

/// Which band a timestamp falls in, or `None` when it was never recorded.
fn band_of(mtime: i64, now: i64, sorted: &[u32]) -> Option<usize> {
    let days = age_days(mtime, now)?;
    Some(
        sorted
            .iter()
            .position(|&edge| days <= i64::from(edge))
            .unwrap_or(sorted.len()),
    )
}

/// One band per node, for painting a tree by age.
///
/// A **file** takes its own band, whatever its size: a tile is coloured by
/// what it is, and a zero-byte file is as old as it is old.
///
/// A **directory** takes the band its subtree's *median byte* sits in — sort
/// every byte below it by age and look at the one in the middle. Two other
/// answers were rejected. Its own `mtime` is the one the module header warns
/// about: it moves when an entry is added beside it and says nothing about
/// what is inside, so a folder of decade-old files reads as fresh because
/// something was deleted from it last week. The band holding the *most* bytes
/// is jumpy in the other direction — a folder split 51/49 between new and
/// ancient is painted entirely as one of them, and the colour flips on a
/// single file. The median moves smoothly and its claim is one a reader can
/// check: half of these bytes are older than this.
///
/// `None` means there is nothing to be old: an empty folder, or one whose
/// files carry no recorded time. Nothing is a safer thing to say than a
/// colour, which would be read as a measurement.
///
/// Indexed by node id, so `bands[id as usize]` is that entry's band.
///
/// Costs one `u64` per band per node while it runs: measured on the 412,380
/// entries of `/Applications`, that is **18.9 MB and 3.5 ms** — cheap enough
/// that the desktop could afford to call it per frame, and it caches the
/// result per opened tree anyway because allocating 19 MB on every hover is
/// not something to do for no reason.
pub fn median_bands(tree: &Tree, now: i64, edges: &[u32]) -> Vec<Option<u8>> {
    let sorted = clean_edges(edges);
    let width = sorted.len() + 1;
    let mut hist = vec![0u64; tree.len() * width];

    // Descending, which is what the arena layout buys: every child has a
    // higher index than its parent (invariant 2), so by the time a node is
    // reached every one of its children has already added itself in. One pass,
    // no recursion, no stack.
    for id in (0..tree.len()).rev() {
        let node = tree.node(id as NodeId);
        if node.kind != EntryKind::Dir {
            if let Some(band) = band_of(node.mtime, now, &sorted) {
                hist[id * width + band] += node.own_alloc;
            }
        }
        let parent = node.parent;
        if parent == Tree::NO_PARENT {
            continue;
        }
        let (before, after) = hist.split_at_mut(id * width);
        let target = parent as usize * width;
        for band in 0..width {
            before[target + band] += after[band];
        }
    }

    (0..tree.len())
        .map(|id| {
            let node = tree.node(id as NodeId);
            if node.kind != EntryKind::Dir {
                return band_of(node.mtime, now, &sorted).map(|b| b as u8);
            }
            median_of(&hist[id * width..(id + 1) * width])
        })
        .collect()
}

/// The band the middle byte of a histogram sits in.
///
/// `>=` against half the total, so the band that contains the midpoint wins
/// rather than the one after it. An exact half-and-half split therefore lands
/// on the *younger* of the two, which is the conservative direction: calling
/// something colder than it is invites deleting it.
fn median_of(bands: &[u64]) -> Option<u8> {
    let total: u64 = bands.iter().sum();
    if total == 0 {
        return None;
    }
    let mut seen = 0u64;
    for (index, &bytes) in bands.iter().enumerate() {
        seen += bytes;
        // Doubling rather than halving the total: with an odd total, halving
        // truncates and the midpoint drifts a band younger on small folders.
        if seen * 2 >= total {
            return Some(index as u8);
        }
    }
    None
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

    // --------------------------------------------------- bands, for painting

    fn bands_of(children: Vec<ImportedNode>) -> Vec<Option<u8>> {
        median_bands(&tree_of(children), NOW, DEFAULT_EDGES)
    }

    /// A tile is coloured by what it is. Size weights a *folder's* answer, not
    /// a file's own — otherwise an empty file would have no colour at all.
    #[test]
    fn a_file_takes_its_own_band_however_small() {
        let bands = bands_of(vec![file_aged("empty-but-ancient", 3_000, 0)]);
        assert_eq!(bands[1], Some(5), "the open-ended oldest band");
    }

    /// The heart of it: the middle byte decides, not the biggest pile. Here
    /// the largest single band is the newest one and the median is not — a
    /// rule that painted the folder by its dominant band would disagree.
    #[test]
    fn a_folder_takes_the_band_its_middle_byte_sits_in() {
        let mut folder = ImportedNode::dir("mixed");
        folder.children = vec![
            file_aged("new", 1, 40),
            file_aged("a-month-and-a-half", 45, 25),
            file_aged("ancient", 3_000, 35),
        ];
        let bands = median_bands(&tree_of(vec![folder]), NOW, DEFAULT_EDGES);
        assert_eq!(
            bands[1],
            Some(2),
            "40 bytes new, then 25 reaching past the midpoint in the 31..=90 band"
        );
    }

    /// The claim the colour makes is "half of these bytes are older than
    /// this", so the count is of bytes and not of files. Two hundred fresh
    /// small files must not outvote one ancient disk image.
    #[test]
    fn a_folder_is_weighted_by_bytes_not_by_file_count() {
        let mut folder = ImportedNode::dir("downloads");
        folder.children = (0..200)
            .map(|i| file_aged(&format!("note{i}"), 1, 10))
            .chain(std::iter::once(file_aged("image.dmg", 3_000, 1_000_000)))
            .collect();
        let bands = median_bands(&tree_of(vec![folder]), NOW, DEFAULT_EDGES);
        assert_eq!(bands[1], Some(5));
    }

    /// The rule the module header warns about, as a test. A directory's own
    /// mtime moves when anything is added beside it, so a folder of ancient
    /// files must not read as fresh because it was touched yesterday.
    #[test]
    fn a_folders_own_mtime_does_not_colour_it() {
        let mut folder = ImportedNode::dir("archive");
        folder.mtime = NOW - DAY;
        folder.children = vec![file_aged("old", 3_000, 100)];
        let bands = median_bands(&tree_of(vec![folder]), NOW, DEFAULT_EDGES);
        assert_eq!(bands[1], Some(5), "the files decide, not the folder");
    }

    /// Bytes arrive from any depth, not only from direct children.
    #[test]
    fn a_folder_counts_everything_beneath_it_not_just_its_children() {
        let mut deep = ImportedNode::dir("deep");
        deep.children = vec![file_aged("buried", 3_000, 900)];
        let mut middle = ImportedNode::dir("middle");
        middle.children = vec![deep, file_aged("beside", 1, 100)];

        let tree = tree_of(vec![middle]);
        let bands = median_bands(&tree, NOW, DEFAULT_EDGES);
        assert_eq!(bands[1], Some(5), "middle");
        assert_eq!(bands[0], Some(5), "and the root above it");
    }

    /// An even split has no middle byte to speak of. Landing on the younger
    /// band is the conservative direction: a colour that says "cold" is the
    /// one somebody acts on by deleting.
    #[test]
    fn an_even_split_lands_on_the_younger_band() {
        let mut folder = ImportedNode::dir("half-and-half");
        folder.children = vec![file_aged("new", 1, 500), file_aged("old", 3_000, 500)];
        let bands = median_bands(&tree_of(vec![folder]), NOW, DEFAULT_EDGES);
        assert_eq!(bands[1], Some(0));
    }

    /// Nothing to be old, so nothing is said. A colour would be read as a
    /// measurement of something that was never measured.
    #[test]
    fn a_folder_with_no_dated_bytes_has_no_band() {
        let empty = ImportedNode::dir("empty");

        let mut undated = ImportedNode::file("mystery", 900, 900);
        undated.mtime = 0;
        let mut unknown_only = ImportedNode::dir("undated");
        unknown_only.children = vec![undated];

        let mut zero_bytes = ImportedNode::dir("zero-bytes");
        zero_bytes.children = vec![file_aged("placeholder", 10, 0)];

        let tree = tree_of(vec![empty, unknown_only, zero_bytes]);
        let bands = median_bands(&tree, NOW, DEFAULT_EDGES);
        assert_eq!(bands[1], None, "empty");
        assert_eq!(bands[2], None, "nothing dated inside");
        assert_eq!(bands[3], None, "dated, but no bytes to weigh");
    }

    /// A file whose time was never recorded is not evidence of age, so it
    /// must not pull a folder's answer either way.
    #[test]
    fn undated_files_do_not_shift_a_folders_band() {
        let mut undated = ImportedNode::file("mystery", 10_000, 10_000);
        undated.mtime = 0;
        let mut folder = ImportedNode::dir("mostly-unknown");
        folder.children = vec![undated, file_aged("known", 3_000, 10)];

        let bands = median_bands(&tree_of(vec![folder]), NOW, DEFAULT_EDGES);
        assert_eq!(bands[1], Some(5), "the one dated file decides alone");
    }

    // ------------------------------------------------ profile of a subtree

    /// The legend has to describe the folder on screen. Counting the whole
    /// scan would put bytes in the key that are nowhere in the picture.
    #[test]
    fn a_subtree_profile_counts_only_that_subtree() {
        let mut inside = ImportedNode::dir("inside");
        inside.children = vec![file_aged("mine", 3_000, 700)];
        let tree = tree_of(vec![inside, file_aged("outside", 1, 999_000)]);

        let subtree = tree.children(tree.root()).next().unwrap();
        let profile = age_profile_at(&tree, subtree, NOW, DEFAULT_EDGES);
        assert_eq!(profile.buckets.last().unwrap().size, 700);
        let total: u64 = profile.buckets.iter().map(|b| b.size).sum();
        assert_eq!(total, 700, "the sibling's 999,000 bytes are not in view");
    }
}

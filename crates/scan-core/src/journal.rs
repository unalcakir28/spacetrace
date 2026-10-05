//! Where a later scan of the same root can ask the filesystem what changed.
//!
//! A scan records a **cursor**: a position in the volume's change journal,
//! taken before the walk starts, so that everything which changes from then
//! on is in the journal after it — whether or not this walk happened to see
//! it. The next scan of the root replays the journal from there and reads
//! again only what changed (`rescan.rs`).
//!
//! The cursor is stored as an opaque string, versioned by its first word, so
//! the store never has to know which journal it came from. FSEvents on macOS
//! is the only one today. The shape is meant to hold the others: a Windows
//! USN cursor is a volume serial, a journal id and a next USN; a Linux one
//! would be whatever a resident watcher (the agent) records, since fanotify
//! keeps no history of its own.
//!
//! **Everything that would make the journal's answer wrong is in the
//! cursor**, and a mismatch is a full scan rather than a guess: the volume's
//! journal identity (a recreated or purged journal gets a new one), the
//! root's own inode (a root replaced by a rename changes no path under it),
//! and the options that decide what a tree holds (a base scanned without
//! `node_modules` cannot supply it to a scan that wants it).

use std::fmt;
use std::path::Path;
use std::time::Duration;

use crate::meta::RawMeta;
use crate::scan::{ScanOptions, ScanProgress};

/// How a tree was produced, recorded beside the snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Rescan {
    /// Every directory was read, and nobody asked otherwise: there was no
    /// previous scan of the root, a full scan was asked for, or this
    /// platform has no journal to ask.
    #[default]
    Full,
    /// Built from the previous snapshot of the root, reading again only what
    /// the journal said had changed.
    Incremental(Incremental),
    /// An incremental rescan was tried and refused for this reason, and
    /// every directory was read instead. Never an error: the full scan is the
    /// right answer, it only costs more.
    Fallback(Fallback),
}

impl Rescan {
    /// Which of the three it was, without an incremental scan's detail.
    pub fn kind(&self) -> RescanKind {
        match self {
            Rescan::Full => RescanKind::Full,
            Rescan::Incremental(_) => RescanKind::Incremental,
            Rescan::Fallback(reason) => RescanKind::Fallback(*reason),
        }
    }

    /// [`RescanKind::record`] of [`Rescan::kind`].
    pub fn record(&self) -> String {
        self.kind().record()
    }

    pub fn is_incremental(&self) -> bool {
        matches!(self, Rescan::Incremental(_))
    }
}

/// How a stored scan was made: what the store keeps and hands back, and what
/// the agent reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescanKind {
    Full,
    Incremental,
    Fallback(Fallback),
}

impl RescanKind {
    /// The wire form, in the store and in JSON: `full`, `incremental`, or
    /// `fallback:` followed by [`Fallback::code`].
    pub fn record(self) -> String {
        match self {
            RescanKind::Full => "full".to_string(),
            RescanKind::Incremental => "incremental".to_string(),
            RescanKind::Fallback(reason) => format!("fallback:{}", reason.code()),
        }
    }

    /// [`RescanKind::record`], read back; `None` for text it did not write —
    /// a reason a newer build added, say.
    pub fn parse(text: &str) -> Option<RescanKind> {
        match text {
            "full" => Some(RescanKind::Full),
            "incremental" => Some(RescanKind::Incremental),
            _ => Fallback::from_code(text.strip_prefix("fallback:")?).map(RescanKind::Fallback),
        }
    }
}

impl fmt::Display for RescanKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.record())
    }
}

/// What an incremental rescan did instead of reading everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Incremental {
    /// Changes the journal reported under the root.
    pub events: u64,
    /// How far back the cursor was, in the journal's own units — FSEvents
    /// event ids, which count every change on the system and not only the
    /// ones under this root. The cost of asking grows with this, not with
    /// how much changed.
    pub distance: u64,
    /// Time spent asking the journal.
    pub replay_ms: u64,
    /// Time spent loading and checking the previous snapshot.
    pub load_ms: u64,
    /// Directories read again from the disk.
    pub dirs_listed: u64,
    /// Entries taken over from the previous snapshot without being read.
    pub entries_reused: u64,
}

/// Why an incremental rescan was not done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// The previous snapshot has no cursor: it was saved before cursors were
    /// kept, taken on a filesystem with no usable journal, or arrived from
    /// another machine.
    NoCursor,
    /// The previous snapshot was imported from another tool's export.
    ImportedBase,
    /// The previous snapshot is dated in the future; the clock moved.
    FutureBase,
    /// The previous snapshot does not match its own digest.
    BaseDamaged,
    /// The previous snapshot was scanned with different options.
    OptionsChanged,
    /// The root's volume, or its journal, is not the one the cursor names —
    /// another disk, or a journal discarded and recreated since.
    OtherVolume,
    /// The root directory itself was replaced since the previous scan.
    RootReplaced,
    /// The cursor is not one this build can read, or points past the
    /// journal's present.
    StaleCursor,
    /// Replaying the journal took longer than the last full scan did.
    Deadline,
    /// The journal reported that it lost track — dropped events, a wrapped
    /// counter, a volume mounted or unmounted under the root.
    EventsLost,
    /// The journal reported a path that could not be placed under the root.
    EventOutsideRoot,
    /// The journal could not be asked at all.
    ReplayFailed,
    /// More paths changed than an incremental scan is worth holding in
    /// memory; reading their directories would cost close to a full scan.
    TooManyChanges,
    /// The previous scan is older than the journal can be trusted to cover
    /// (`rescan::MAX_CURSOR_AGE`).
    TooOld,
}

impl Fallback {
    /// Every reason, for reading a code back.
    const ALL: [Fallback; 14] = [
        Fallback::NoCursor,
        Fallback::ImportedBase,
        Fallback::FutureBase,
        Fallback::BaseDamaged,
        Fallback::OptionsChanged,
        Fallback::OtherVolume,
        Fallback::RootReplaced,
        Fallback::StaleCursor,
        Fallback::Deadline,
        Fallback::EventsLost,
        Fallback::EventOutsideRoot,
        Fallback::ReplayFailed,
        Fallback::TooManyChanges,
        Fallback::TooOld,
    ];

    /// The one table: each reason's stable code and the sentence a person
    /// reads, side by side so neither can be added without the other.
    fn info(self) -> (&'static str, &'static str) {
        match self {
            Fallback::NoCursor => ("no-cursor", "the previous snapshot has no journal position"),
            Fallback::ImportedBase => ("imported-base", "the previous snapshot was imported"),
            Fallback::FutureBase => (
                "future-base",
                "the previous snapshot is dated in the future",
            ),
            Fallback::BaseDamaged => (
                "base-damaged",
                "the previous snapshot does not match its digest",
            ),
            Fallback::OptionsChanged => (
                "options-changed",
                "the previous snapshot used other scan options",
            ),
            Fallback::OtherVolume => (
                "other-volume",
                "the root's volume or its journal has changed",
            ),
            Fallback::RootReplaced => ("root-replaced", "the root directory was replaced"),
            Fallback::StaleCursor => ("stale-cursor", "the journal position is no longer valid"),
            Fallback::Deadline => (
                "deadline",
                "the journal took longer to answer than a full scan",
            ),
            Fallback::EventsLost => ("events-lost", "the journal lost track of changes"),
            Fallback::EventOutsideRoot => (
                "event-outside-root",
                "the journal reported a change it could not place",
            ),
            Fallback::ReplayFailed => ("replay-failed", "the journal could not be read"),
            Fallback::TooManyChanges => (
                "too-many-changes",
                "more paths changed than an incremental scan is worth",
            ),
            Fallback::TooOld => (
                "too-old",
                "the previous scan is older than the journal is trusted to cover",
            ),
        }
    }

    /// A short stable name, for the store and for machine-readable output.
    pub fn code(self) -> &'static str {
        self.info().0
    }

    /// [`Fallback::code`], read back.
    pub fn from_code(code: &str) -> Option<Fallback> {
        Fallback::ALL
            .into_iter()
            .find(|reason| reason.code() == code)
    }
}

impl fmt::Display for Fallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.info().1)
    }
}

/// One change a journal reported, in terms that do not depend on which
/// journal it was.
///
/// The one shape `rescan.rs` reads. FSEvents translates into it
/// (`fsevents::change`), and a Windows USN reader or a Linux watcher would do
/// the same, so a new journal never touches the code that decides what to
/// read again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
// Built only by a journal, and macOS has the only one yet.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct Change {
    /// The path as the journal spells it, absolute.
    pub path: Vec<u8>,
    pub kind: ChangeKind,
    /// Whether the entry is a directory, as far as the journal knows.
    pub is_dir: bool,
}

/// What happened at a [`Change`]'s path. Where a journal coalesced several
/// things into one record, it reports the one listed last here that applies:
/// each is a stronger claim about what must be read again than those above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum ChangeKind {
    /// Its contents or metadata changed; the entry itself stayed.
    Modified,
    /// It appeared.
    Created,
    /// It moved, to or from this path.
    Renamed,
    /// It went.
    Removed,
    /// Something changed at or below it and the journal kept no detail: read
    /// all of it again.
    SubtreeUnknown,
    /// The journal lost track — dropped records, a wrapped counter, a volume
    /// mounted or unmounted — and nothing it says for this stretch can be
    /// trusted.
    Lost,
}

/// Why a journal produced no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum NoAnswer {
    /// The history did not end within the budget.
    Deadline,
    /// The scan was cancelled while it waited.
    Cancelled,
    /// The journal could not be asked at all.
    Failed,
    /// More changed paths than an incremental scan is worth holding.
    TooMany,
}

/// A volume change journal, as a rescan uses one.
///
/// The seam every platform plugs into: [`system`] hands out this platform's,
/// and nothing outside the implementation knows which it is.
///
/// **What a copied subtree may not hold.** The scan-wide counters in
/// `ScanStats` are added up by the walk, so a subtree copied unread adds
/// nothing to them — which is right only because every entry that adds to
/// one also flags its directory against copying: a hardlink, a clone or a
/// file with mapped extents (`hardlinks_deduped`, `clones_deduped`,
/// `shared_bytes_deduped`, `compressed_*`) is `SHARED`, and a volume whose
/// sharing cannot be seen (`unseen_sharing`) is only ever met at a mount,
/// which is `MOUNT`. One counter has no flag yet: `files_unmapped`, a file
/// on btrfs or XFS whose extents could not be read. It is reachable only on
/// Linux, which has no journal; a Linux journal must flag that file's
/// directory first, or an incremental scan would drop it from the count.
pub(crate) trait Journal: Sync {
    /// The first word of every cursor this journal writes, format version
    /// included. A stored cursor with another is never replayed.
    fn kind(&self) -> &'static str;

    /// The identity of the journal that covers `root` — one that changes
    /// whenever its history is discarded — or `None` where no journal can be
    /// trusted to hold every change made under it from now on.
    fn volume(&self, root: &Path, root_meta: &RawMeta) -> Option<String>;

    /// The newest position the journal has issued.
    fn position(&self) -> u64;

    /// Everything that changed under `root` after position `since`, or why
    /// there is no answer within `budget`. Counts the wait in
    /// `progress.journal_ms` (invariant 8) and stops when cancelled.
    fn replay(
        &self,
        root: &Path,
        since: u64,
        budget: Duration,
        progress: &ScanProgress,
    ) -> Result<Vec<Change>, NoAnswer>;
}

/// This platform's journal, where this build reads one.
#[cfg(target_os = "macos")]
pub(crate) fn system() -> Option<&'static dyn Journal> {
    Some(&crate::fsevents::FsEvents)
}

/// None elsewhere yet: every scan reads everything.
#[cfg(not(target_os = "macos"))]
pub(crate) fn system() -> Option<&'static dyn Journal> {
    None
}

/// A position in a volume's change journal, and what it was taken under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    /// [`Journal::kind`] of the journal that wrote it.
    pub kind: String,
    /// [`Journal::volume`]: the identity of the volume's journal.
    pub volume: String,
    /// The journal position taken before the walk started.
    pub position: u64,
    /// The root directory's inode when the cursor was taken.
    pub root_ino: u64,
    /// [`fingerprint`] of the options the scan ran with.
    pub options: u64,
    /// How long the last scan that read every directory took — what the
    /// replay's budget is made from. Carried forward by every incremental
    /// scan, so a chain of quick ones does not shrink its own budget.
    pub full_ms: u64,
    /// When the cursor was taken, in seconds since the Unix epoch: how old
    /// the stretch of history a replay from it relies on is.
    pub taken_at: u64,
}

impl Cursor {
    /// Space-separated, in a fixed order: kind, volume, position, root inode,
    /// options (hex), full_ms, taken_at. A volume identity never holds a
    /// space — it is a UUID, a serial number — and `decode` refuses one that
    /// would.
    pub fn encode(&self) -> String {
        format!(
            "{} {} {} {} {:016x} {} {}",
            self.kind,
            self.volume,
            self.position,
            self.root_ino,
            self.options,
            self.full_ms,
            self.taken_at
        )
    }

    /// Read a stored cursor back; `None` for anything [`Cursor::encode`] did
    /// not write: a field missing, extra, empty or unparseable. A defaulted
    /// field would be worse than no cursor — position 0 is the whole history,
    /// a defaulted volume a wrong one.
    pub fn decode(text: &str) -> Option<Cursor> {
        let fields: Vec<&str> = text.split(' ').collect();
        let [kind, volume, position, root_ino, options, full_ms, taken_at] = fields[..] else {
            return None;
        };
        if kind.is_empty() || volume.is_empty() {
            return None;
        }
        Some(Cursor {
            kind: kind.to_string(),
            volume: volume.to_string(),
            position: position.parse().ok()?,
            root_ino: root_ino.parse().ok()?,
            options: u64::from_str_radix(options, 16).ok()?,
            full_ms: full_ms.parse().ok()?,
            taken_at: taken_at.parse().ok()?,
        })
    }
}

/// A cursor for a scan of `root` about to start, or `None` where there is no
/// journal that can be trusted to hold every change made from now on.
///
/// Its `full_ms` is left at 0: how long the walk takes is known only after it,
/// and whoever finishes the scan fills it in.
pub(crate) fn take(
    journal: Option<&dyn Journal>,
    root: &Path,
    root_meta: &RawMeta,
    opts: &ScanOptions,
) -> Option<Cursor> {
    let journal = journal?;
    if root_meta.kind != crate::EntryKind::Dir {
        return None;
    }
    // The volume first, the position second: if the history were discarded
    // in between, the old identity would be stored beside a new position and
    // the next scan would notice nothing. The other order stores a position
    // older than the identity it names, which costs a few extra records and
    // loses none.
    let volume = journal.volume(root, root_meta)?;
    Some(Cursor {
        kind: journal.kind().to_string(),
        volume,
        position: journal.position(),
        root_ino: root_meta.ino,
        options: fingerprint(opts),
        full_ms: 0,
        taken_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
    })
}

/// The options that decide what a tree holds, reduced to one number.
///
/// Thread count and the capacity hint are left out: they change how fast a
/// tree is built, never what is in it. Everything else is in, including
/// whether mounts are approached with a deadline — that decides whether a
/// dead one is an error or a hang.
///
/// FNV-1a over a canonical encoding, written out here rather than taken from
/// `std::hash`: the standard hasher's algorithm is unspecified and may change
/// between Rust releases, which would turn a compiler upgrade into a full
/// scan of every root. Not a defence against anything — two option sets
/// colliding would need an adversary who controls the agent's configuration.
pub(crate) fn fingerprint(opts: &ScanOptions) -> u64 {
    let mut excluded: Vec<&str> = opts.exclude_names.iter().map(String::as_str).collect();
    excluded.sort_unstable();
    excluded.dedup();
    let mut hash = Fnv::new();
    hash.field(b"exclude");
    for name in excluded {
        hash.field(name.as_bytes());
    }
    hash.field(b"one_filesystem");
    hash.field(&[u8::from(opts.one_filesystem)]);
    hash.field(b"max_depth");
    match opts.max_depth {
        Some(depth) => hash.field(&(depth as u64).to_le_bytes()),
        None => hash.field(b"-"),
    }
    hash.field(b"dedupe");
    hash.field(&[
        u8::from(opts.dedupe_hardlinks),
        u8::from(opts.dedupe_clones),
        u8::from(opts.mount_timeout.is_some()),
    ]);
    hash.0
}

/// 64-bit FNV-1a, with each field length-prefixed so two lists that
/// concatenate alike — `["ab", "c"]` and `["a", "bc"]` — do not hash alike.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn field(&mut self, bytes: &[u8]) {
        for &byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor() -> Cursor {
        Cursor {
            kind: "fsevents1".to_string(),
            volume: "8A1B2C3D-0000-4000-8000-123456789ABC".to_string(),
            position: 803_031_221,
            root_ino: 42,
            options: 0x0123_4567_89ab_cdef,
            full_ms: 1431,
            taken_at: 1_791_100_000,
        }
    }

    #[test]
    fn a_cursor_survives_being_stored() {
        let c = cursor();
        assert_eq!(Cursor::decode(&c.encode()), Some(c));
    }

    /// Anything not written by this format must read as "no cursor", never
    /// as a cursor with a field defaulted: a defaulted position of 0 would be
    /// a replay of the whole history, and a defaulted volume a wrong one.
    #[test]
    fn a_cursor_with_anything_missing_or_extra_is_refused() {
        let good = cursor().encode();
        for bad in [
            String::new(),
            good.replace(" 803031221", ""),
            good.replace(" 42 ", " forty-two "),
            good.replace("8A1B2C3D-0000-4000-8000-123456789ABC", ""),
            good.replace("fsevents1", ""),
            good.clone() + " 1",
            good.replace(' ', "  "),
            good.replace("0123456789abcdef", "not-hex"),
        ] {
            assert_eq!(Cursor::decode(&bad), None, "{bad:?} was accepted");
        }
    }

    #[test]
    fn options_that_change_the_tree_change_the_fingerprint() {
        let base = ScanOptions::default();
        let same = ScanOptions {
            threads: Some(3),
            expected_entries: Some(10),
            ..ScanOptions::default()
        };
        assert_eq!(
            fingerprint(&base),
            fingerprint(&same),
            "threads and the size hint do not change what a tree holds"
        );

        let changed = [
            ScanOptions {
                exclude_names: vec!["node_modules".into()],
                ..ScanOptions::default()
            },
            ScanOptions {
                one_filesystem: true,
                ..ScanOptions::default()
            },
            ScanOptions {
                max_depth: Some(3),
                ..ScanOptions::default()
            },
            ScanOptions {
                dedupe_hardlinks: false,
                ..ScanOptions::default()
            },
            ScanOptions {
                dedupe_clones: false,
                ..ScanOptions::default()
            },
            ScanOptions {
                mount_timeout: None,
                ..ScanOptions::default()
            },
        ];
        for opts in &changed {
            assert_ne!(fingerprint(&base), fingerprint(opts), "{opts:?}");
        }
    }

    #[test]
    fn the_exclude_list_is_a_set() {
        let one = ScanOptions {
            exclude_names: vec!["a".into(), "b".into(), "a".into()],
            ..ScanOptions::default()
        };
        let two = ScanOptions {
            exclude_names: vec!["b".into(), "a".into()],
            ..ScanOptions::default()
        };
        let joined = ScanOptions {
            exclude_names: vec!["ab".into()],
            ..ScanOptions::default()
        };
        assert_eq!(fingerprint(&one), fingerprint(&two));
        assert_ne!(fingerprint(&two), fingerprint(&joined));
    }

    /// Pinned, because the number is stored: if the encoding changes every
    /// stored cursor stops matching and every root takes one full scan. That
    /// may be worth it, but it should be a decision.
    #[test]
    fn the_fingerprint_is_pinned() {
        assert_eq!(fingerprint(&ScanOptions::default()), PINNED_DEFAULT);
    }

    const PINNED_DEFAULT: u64 = 0xe16e_c9f8_9b00_c1a1;

    /// Every reason's code is distinct and reads back as that reason, and so
    /// does every record: they are stored, so a typo would turn a reason
    /// into "unknown" on the next read.
    #[test]
    fn every_record_reads_back_as_itself() {
        let codes: std::collections::HashSet<&str> =
            Fallback::ALL.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), Fallback::ALL.len(), "two reasons share a code");
        let kinds = Fallback::ALL
            .into_iter()
            .map(RescanKind::Fallback)
            .chain([RescanKind::Full, RescanKind::Incremental]);
        for kind in kinds {
            assert_eq!(RescanKind::parse(&kind.record()), Some(kind));
        }
        assert_eq!(RescanKind::parse("fallback:not-a-reason"), None);
        assert_eq!(RescanKind::parse("partial"), None);
    }
}

//! The NTFS master file table, read as a whole and turned into directory
//! listings — the Windows fast path (TODO B4).
//!
//! **Why the MFT and not the USN enumeration.** `FSCTL_ENUM_USN_DATA`
//! reconstructs the tree but carries no sizes at all, so it would still need
//! the open handle per entry that this exists to remove (`meta.rs`, ~+36%).
//! Every fact the walk asks for lives in the MFT records themselves: names and
//! parents in `$FILE_NAME`, the modification time in `$STANDARD_INFORMATION`,
//! the length and the allocation in the header of the unnamed `$DATA`. Read
//! the table once, front to back, and the whole volume is answered.
//!
//! **Why this file knows nothing about Windows.** Nothing that can go wrong
//! here is a type error — an unapplied fixup is silently wrong every 512
//! bytes, a `$DATA` split across extension records loses its size, a DOS name
//! doubles a file — so the parser has to be run against volumes a real NTFS
//! wrote. It reads any `Read + Seek`, and the tests feed it images ntfs-3g
//! made and compare every entry with what the ntfs-3g mount reported
//! (`scripts/ntfs-fixtures.sh`). The part that opens `\\.\C:` is `volume.rs`,
//! and it is the part only Windows CI can check.
//!
//! **What each field means is the normal Windows walk's definition, not
//! NTFS's.** That walk is the reference this path is compared against, field
//! for field, so the rules below are spelled out where they are applied:
//! `size` is the unnamed stream's length, `alloc` is what
//! `FILE_STANDARD_INFO.AllocationSize` reports, the link count leaves out DOS
//! names, and the NTFS metafiles below record 16 are not entries at all.

#[cfg(test)]
mod tests;

use std::io::{self, Read, Seek, SeekFrom};

use crate::meta::{filetime_to_unix, EntryKind};
use crate::scan::MAX_REPORTED_ERRORS;

/// The root directory's record number, on every NTFS volume.
pub(crate) const ROOT_RECORD: u64 = 5;

/// Records below this one are the metafiles (`$MFT`, `$Bitmap`, `$Extend`,
/// …). Windows leaves them out of every directory listing, so the walk never
/// sees them, and neither may this. `$Extend`'s own children are hidden with
/// it, since they are reached only through it.
const FIRST_USER_RECORD: u64 = 16;

/// The update sequence stride. Always 512, whatever the sector size: NTFS
/// protects each 512-byte block of a record, not each physical sector.
const FIXUP_STRIDE: usize = 512;

/// How much of the table is read per call. A multiple of every record size
/// NTFS uses, and large enough that the read, not the call, is the cost.
const CHUNK_BYTES: usize = 1 << 20;

/// The low 48 bits of a file reference are the record number, the high 16 the
/// sequence number the record had when the reference was made.
const RECORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

const ATTR_STANDARD_INFORMATION: u32 = 0x10;
const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_INDEX_ALLOCATION: u32 = 0xA0;
const ATTR_REPARSE_POINT: u32 = 0xC0;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// Record header flags.
const RECORD_IN_USE: u16 = 0x0001;
const RECORD_IS_DIRECTORY: u16 = 0x0002;

/// Attribute flags that make the "total allocated" field the one that counts.
const ATTRIBUTE_COMPRESSED: u16 = 0x00FF;
const ATTRIBUTE_SPARSE: u16 = 0x8000;

/// `$FILE_NAME` namespace of a name that exists only as the 8.3 alias of a
/// long one. Windows lists the long name and reports the short one beside it;
/// counted on its own, every file with an alias would appear twice.
const NAMESPACE_DOS: u8 = 2;

/// Reparse tags whose bit 29 is set are name surrogates — symlinks, junctions,
/// volume mount points. Rust's `FileType::is_symlink` on Windows is exactly
/// this bit, so this is what decides `EntryKind::Symlink` on both paths.
const REPARSE_NAME_SURROGATE: u32 = 0x2000_0000;

/// UTF-16LE names compared against attribute names.
const NAME_I30: &[u8] = &[b'$', 0, b'I', 0, b'3', 0, b'0', 0];
/// The stream a WOF-compressed file (`compact /exe`, CompactOS) keeps its
/// bytes in. The unnamed stream of such a file is a sparse placeholder that
/// allocates nothing, so without this its blocks would be charged to nobody.
const NAME_WOF: &[u8] = &[
    b'W', 0, b'o', 0, b'f', 0, b'C', 0, b'o', 0, b'm', 0, b'p', 0, b'r', 0, b'e', 0, b's', 0, b's',
    0, b'e', 0, b'd', 0, b'D', 0, b'a', 0, b't', 0, b'a', 0,
];

/// Where the table is and how it is cut up, from the boot sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub bytes_per_cluster: u64,
    pub bytes_per_record: u64,
    /// The cluster `$MFT`'s record 0 starts at.
    pub mft_lcn: u64,
}

impl Geometry {
    /// Parse an NTFS boot sector. Refuses anything that is not one, since a
    /// wrong geometry reads garbage that can look like records.
    pub(crate) fn from_boot_sector(boot: &[u8]) -> io::Result<Geometry> {
        if boot.len() < 512 || &boot[3..11] != b"NTFS    " || boot[510..512] != [0x55, 0xAA] {
            return Err(invalid("not an NTFS boot sector"));
        }
        let bytes_per_sector = u64::from(u16::from_le_bytes([boot[0x0B], boot[0x0C]]));
        if !bytes_per_sector.is_power_of_two() || !(256..=4096).contains(&bytes_per_sector) {
            return Err(invalid("implausible sector size"));
        }
        // Above 0x80 the field is a negative power of two, which is how
        // clusters past 64 KiB are written (up to 2 MiB since Windows 10).
        let raw = boot[0x0D];
        let sectors_per_cluster = match raw {
            0 => return Err(invalid("zero sectors per cluster")),
            1..=0x80 => u64::from(raw),
            _ => 1u64 << (256 - u32::from(raw)).min(31),
        };
        let bytes_per_cluster = bytes_per_sector * sectors_per_cluster;
        if !bytes_per_cluster.is_power_of_two() || bytes_per_cluster > 2 << 20 {
            return Err(invalid("implausible cluster size"));
        }
        // Positive: clusters per record. Negative: the record is 2^-n bytes,
        // which is the usual case — a 1 KiB record in a 4 KiB cluster.
        let per_record = boot[0x40] as i8;
        let bytes_per_record = match per_record {
            1..=127 => u64::from(per_record as u8) * bytes_per_cluster,
            -31..=-1 => 1u64 << -i32::from(per_record),
            _ => return Err(invalid("implausible record size")),
        };
        if !bytes_per_record.is_power_of_two()
            || !(FIXUP_STRIDE as u64..=64 * 1024).contains(&bytes_per_record)
        {
            return Err(invalid("implausible record size"));
        }
        let mft_lcn = u64::from_le_bytes(boot[0x30..0x38].try_into().expect("eight bytes"));
        Ok(Geometry {
            bytes_per_cluster,
            bytes_per_record,
            mft_lcn,
        })
    }
}

/// Read the boot sector at offset 0 of `volume`.
///
/// 4096 bytes rather than 512: a volume handle on Windows refuses a read that
/// is not a whole number of sectors, and 4096 is one on every disk in use.
pub(crate) fn read_geometry<R: Read + Seek>(volume: &mut R) -> io::Result<Geometry> {
    let mut boot = vec![0u8; 4096];
    volume.seek(SeekFrom::Start(0))?;
    volume.read_exact(&mut boot)?;
    Geometry::from_boot_sector(&boot)
}

/// One run of a non-resident attribute: `len` clusters from `vcn` on, at
/// `lcn`, or a hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    pub vcn: u64,
    pub lcn: Option<u64>,
    pub len: u64,
}

/// Decode a mapping-pairs array. `None` for anything malformed: a run list
/// is the one thing here that sends reads elsewhere on the disk, so a bad one
/// must stop the read rather than be guessed at.
pub(crate) fn decode_runs(bytes: &[u8], first_vcn: u64) -> Option<Vec<Run>> {
    let mut runs = Vec::new();
    let (mut vcn, mut lcn, mut pos) = (first_vcn, 0i64, 0usize);
    loop {
        let header = *bytes.get(pos)?;
        if header == 0 {
            return Some(runs);
        }
        let len_size = usize::from(header & 0x0F);
        let off_size = usize::from(header >> 4);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return None;
        }
        let len = unsigned(bytes.get(pos + 1..pos + 1 + len_size)?);
        if len == 0 {
            return None;
        }
        // A hole has no offset, and the next run's delta is taken from the
        // last real one.
        let at = if off_size == 0 {
            None
        } else {
            let field = bytes.get(pos + 1 + len_size..pos + 1 + len_size + off_size)?;
            lcn = lcn.checked_add(signed(field))?;
            Some(u64::try_from(lcn).ok()?)
        };
        runs.push(Run { vcn, lcn: at, len });
        vcn = vcn.checked_add(len)?;
        pos += 1 + len_size + off_size;
    }
}

fn unsigned(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .rev()
        .fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
}

fn signed(bytes: &[u8]) -> i64 {
    let value = unsigned(bytes);
    let bits = bytes.len() * 8;
    if bits == 64 || value & (1 << (bits - 1)) == 0 {
        return value as i64;
    }
    (value | (u64::MAX << bits)) as i64
}

/// Undo the update sequence array in place. `false` when a protected block's
/// last two bytes do not carry the sequence number — the record was torn by a
/// write in flight, or it is not a record.
///
/// This is the step that is silently wrong without it: the real last two
/// bytes of every 512-byte block live in the array, and the record itself
/// carries the sequence number there instead. Skip it and an attribute that
/// crosses a block boundary reads two corrupted bytes, with nothing to say so.
pub(crate) fn apply_fixups(record: &mut [u8]) -> bool {
    let (Some(offset), Some(count)) = (le16(record, 4), le16(record, 6)) else {
        return false;
    };
    let (offset, count) = (usize::from(offset), usize::from(count));
    if count < 2 || (count - 1) * FIXUP_STRIDE != record.len() || offset + 2 * count > record.len()
    {
        return false;
    }
    let usn = [record[offset], record[offset + 1]];
    for block in 1..count {
        let end = block * FIXUP_STRIDE;
        if record[end - 2..end] != usn {
            return false;
        }
        let saved = offset + 2 * block;
        record[end - 2] = record[saved];
        record[end - 1] = record[saved + 1];
    }
    true
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("NTFS: {what}"))
}

/// What an attribute header says about one stream: `(length, allocation)`,
/// with the allocation in `FILE_STANDARD_INFO.AllocationSize`'s terms.
///
/// * **Resident:** the value's length rounded up to eight bytes — what NTFS
///   reports for data that lives inside the record (and what ntfs-3g reports
///   too, which the fixtures check).
/// * **Compressed or sparse:** the "total allocated" field, the clusters that
///   actually exist. The ordinary allocated size spans the holes and the
///   uncompressed units, which is the number invariant 1 refuses to call
///   "what the disk holds".
/// * **Otherwise:** the allocated size, a whole number of clusters.
///
/// `None` unless this instance is the one that carries sizes: a stream split
/// across extension records repeats its header in each, and only the
/// instance starting at VCN 0 has the sizes filled in.
fn stream_sizes(attr: &[u8]) -> Option<(u64, u64)> {
    if attr[8] == 0 {
        let len = u64::from(le32(attr, 0x10)?);
        return Some((len, (len + 7) & !7));
    }
    if le64(attr, 0x10)? != 0 {
        return None;
    }
    let flags = le16(attr, 0x0C)?;
    let length = le64(attr, 0x30)?;
    let alloc = if flags & (ATTRIBUTE_COMPRESSED | ATTRIBUTE_SPARSE) != 0 {
        le64(attr, 0x40)?
    } else {
        le64(attr, 0x28)?
    };
    Some((length, alloc))
}

/// An attribute's name, as raw UTF-16LE.
fn attribute_name(attr: &[u8]) -> Option<&[u8]> {
    let len = usize::from(attr[9]) * 2;
    let at = usize::from(le16(attr, 0x0A)?);
    attr.get(at..at + len)
}

/// A resident attribute's value.
fn resident_value(attr: &[u8]) -> Option<&[u8]> {
    if attr[8] != 0 {
        return None;
    }
    let len = le32(attr, 0x10)? as usize;
    let at = usize::from(le16(attr, 0x14)?);
    attr.get(at..at + len)
}

/// Per-record flags kept after parsing.
const SLOT_IN_USE: u8 = 1;
const SLOT_DIRECTORY: u8 = 2;
const SLOT_SURROGATE: u8 = 4;

/// What the table knows about one base record. Kept small, because there is
/// one per record on the volume, in use or not.
#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    seq: u16,
    flags: u8,
    links: u16,
    mtime: i64,
    size: u64,
    /// What the file occupies outside its record: the unnamed stream plus a
    /// WOF file's backing stream for a file, `$I30`'s index allocation for a
    /// directory. One field for both, because NTFS gives a directory no
    /// unnamed stream and a file no `$I30` — checked on every fixture image
    /// and on a 200,000-file one, where no record had both.
    alloc: u64,
}

impl Slot {
    fn in_use(&self) -> bool {
        self.flags & SLOT_IN_USE != 0
    }

    /// In use and still carrying sequence number `seq`: the record a
    /// reference made with `seq` meant.
    fn is(&self, seq: u16) -> bool {
        self.in_use() && self.seq == seq
    }

    fn is_directory(&self) -> bool {
        self.flags & SLOT_DIRECTORY != 0
    }

    fn is_surrogate(&self) -> bool {
        self.flags & SLOT_SURROGATE != 0
    }
}

/// The base record a record's facts belong to: itself, or the base an
/// extension record names, with the sequence number it names.
#[derive(Debug, Clone, Copy)]
struct Owner {
    id: u32,
    seq: u16,
    is_base: bool,
}

/// A fact found in an extension record, applied to its base record only once
/// the whole table has been read and only if the base still has the sequence
/// number the extension names. An in-use extension of a since-reused base
/// is exactly what a table read while files are being deleted can contain.
#[derive(Debug, Clone, Copy)]
struct Deferred {
    record: u32,
    seq: u16,
    fact: Fact,
}

#[derive(Debug, Clone, Copy)]
enum Fact {
    Data { size: u64, alloc: u64 },
    Alloc(u64),
    Surrogate,
}

/// One name: which record carries it and in which directory. Wide fields
/// first, so the struct packs into 20 bytes; there is one per file.
#[derive(Debug, Clone, Copy)]
struct Name {
    record: u32,
    parent: u32,
    text_off: u32,
    /// The sequence number of the record this name was found in — the base's
    /// own, or the one an extension record says its base has.
    seq: u16,
    parent_seq: u16,
    text_len: u16,
}

/// One instance of a reparse point's data that did not fit in its record,
/// to be read once the table is done. Rare: only a very long link target gets
/// there. An attribute split across extension records leaves one instance per
/// record, each mapping its own range of the data; only together do they say
/// where the tag, at offset 0, is.
#[derive(Debug, Clone)]
struct OutOfLineReparse {
    owner: Owner,
    runs: Vec<Run>,
    /// The data's length, carried by the instance that starts at VCN 0.
    length: Option<u64>,
}

/// One entry of a directory, with every field the walk reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Child<'a> {
    /// The file reference: record number and sequence number. On NTFS this
    /// is exactly the file index `GetFileInformationByHandle` reports, so it
    /// is the identity hardlink deduplication keys on, as the walk does.
    pub reference: u64,
    pub name: &'a str,
    pub kind: EntryKind,
    pub size: u64,
    pub alloc: u64,
    pub mtime: i64,
    /// Names this file has, leaving out DOS aliases.
    pub links: u32,
}

/// The whole table, reduced to what directory listings need.
#[derive(Debug, Clone, Default)]
pub(crate) struct Table {
    slots: Vec<Slot>,
    /// Valid names only, grouped by parent: `first[d]..first[d + 1]` are the
    /// entries of directory `d`, in table order.
    names: Vec<Name>,
    first: Vec<u32>,
    text: String,
    /// Records that were in use and could not be read, by number; no more
    /// than an error report holds, see `bad_count`.
    pub bad_records: Vec<u64>,
    pub bad_count: u64,
}

impl Table {
    /// The entries of the directory `reference` names, in table order. Only
    /// the record number is read; a reference's sequence number is checked by
    /// [`Table::is_directory`], where it comes from outside the table.
    pub(crate) fn children(&self, reference: u64) -> impl Iterator<Item = Child<'_>> + '_ {
        self.entries(reference).iter().map(move |name| {
            let slot = &self.slots[name.record as usize];
            let kind = if slot.is_surrogate() {
                EntryKind::Symlink
            } else if slot.is_directory() {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            let from = name.text_off as usize;
            Child {
                reference: u64::from(name.record) | (u64::from(slot.seq) << 48),
                name: &self.text[from..from + usize::from(name.text_len)],
                kind,
                // A directory — a junction included — has no unnamed stream.
                size: if slot.is_directory() { 0 } else { slot.size },
                alloc: slot.alloc,
                mtime: slot.mtime,
                links: u32::from(slot.links),
            }
        })
    }

    /// How many bytes the names of directory `reference`'s entries take.
    pub(crate) fn names_len(&self, reference: u64) -> usize {
        self.entries(reference)
            .iter()
            .map(|name| usize::from(name.text_len))
            .sum()
    }

    /// The names listed in directory `reference`.
    fn entries(&self, reference: u64) -> &[Name] {
        let record = (reference & RECORD_MASK) as usize;
        match (self.first.get(record), self.first.get(record + 1)) {
            (Some(&from), Some(&to)) => &self.names[from as usize..to as usize],
            _ => &[],
        }
    }

    /// Whether `reference` names an in-use directory of this table, sequence
    /// number included — the check a root taken from a handle must pass
    /// before its subtree is believed.
    pub(crate) fn is_directory(&self, reference: u64) -> bool {
        self.slots
            .get((reference & RECORD_MASK) as usize)
            .is_some_and(|slot| slot.is((reference >> 48) as u16) && slot.is_directory())
    }

    /// How many records the table has room for, in use or not.
    pub(crate) fn capacity(&self) -> usize {
        self.slots.len()
    }
}

/// Accumulates the table while records stream past.
#[derive(Default)]
struct Builder {
    slots: Vec<Slot>,
    names: Vec<Name>,
    text: String,
    deferred: Vec<Deferred>,
    reparse: Vec<OutOfLineReparse>,
    bad_records: Vec<u64>,
    bad_count: u64,
}

/// What became of one record.
enum Parsed {
    /// Read, or not in use.
    Done,
    /// In use, but its fixups did not check out: possibly torn by a write in
    /// flight, so worth reading once more.
    Torn,
    /// In use and malformed.
    Bad,
}

/// Average bytes per name reserved up front. A guess, like the walk's: wrong
/// costs a reallocation, nothing else.
const NAME_BYTES_GUESS: usize = 20;

impl Builder {
    fn with_records(records: usize) -> Self {
        Builder {
            slots: vec![Slot::default(); records],
            names: Vec::with_capacity(records),
            text: String::with_capacity(records * NAME_BYTES_GUESS),
            ..Builder::default()
        }
    }

    fn bad(&mut self, record: u64) {
        self.bad_count += 1;
        if self.bad_records.len() < MAX_REPORTED_ERRORS {
            self.bad_records.push(record);
        }
    }

    /// Parse one record, `buf` exactly one record long. Never panics on bad
    /// input: a record is bytes off a disk.
    fn record(&mut self, number: u64, buf: &mut [u8]) -> Parsed {
        // Never-used records are zeros; anything else that is not a FILE
        // record and claims nothing is skipped the same way. "BAAD" is a
        // record chkdsk found damaged, which is a read error.
        if &buf[0..4] == b"BAAD" {
            return Parsed::Bad;
        }
        if &buf[0..4] != b"FILE" {
            return Parsed::Done;
        }
        // Read before the fixups on purpose: the flag sits in the first
        // block, which fixups do not touch, and a deleted record whose blocks
        // were half overwritten is not an error.
        let Some(flags) = le16(buf, 0x16) else {
            return Parsed::Bad;
        };
        if flags & RECORD_IN_USE == 0 {
            return Parsed::Done;
        }
        if !apply_fixups(buf) {
            return Parsed::Torn;
        }
        if self.attributes(number, flags, buf).is_some() {
            return Parsed::Done;
        }
        // Whatever a malformed base record had already contributed goes with
        // it: out of use, its names no longer count and nothing lists under
        // it. The entry is missing from the tree, and the caller reports it.
        if le64(buf, 0x20) == Some(0) {
            if let Some(slot) = self.slots.get_mut(number as usize) {
                *slot = Slot::default();
            }
        }
        Parsed::Bad
    }

    fn attributes(&mut self, number: u64, flags: u16, buf: &[u8]) -> Option<()> {
        let seq = le16(buf, 0x10)?;
        let base_ref = le64(buf, 0x20)?;
        let base = base_ref & RECORD_MASK;
        let owner = match base {
            0 => Owner {
                id: u32::try_from(number).ok()?,
                seq,
                is_base: true,
            },
            _ => Owner {
                id: u32::try_from(base).ok()?,
                seq: (base_ref >> 48) as u16,
                is_base: false,
            },
        };
        let slot = self.slots.get_mut(owner.id as usize)?;
        if owner.is_base {
            slot.seq = seq;
            slot.flags |= SLOT_IN_USE;
            if flags & RECORD_IS_DIRECTORY != 0 {
                slot.flags |= SLOT_DIRECTORY;
            }
        }
        for attr in attributes(buf)? {
            let attr = attr?;
            let fact = match le32(attr, 0)? {
                ATTR_STANDARD_INFORMATION if owner.is_base => {
                    let value = resident_value(attr)?;
                    self.slots[owner.id as usize].mtime = filetime_to_unix(le64(value, 0x08)?);
                    None
                }
                ATTR_FILE_NAME => {
                    self.name(owner, attr)?;
                    None
                }
                ATTR_DATA => data_fact(attr)?,
                ATTR_INDEX_ALLOCATION if attribute_name(attr)? == NAME_I30 => {
                    stream_sizes(attr).map(|(_, alloc)| Fact::Alloc(alloc))
                }
                ATTR_REPARSE_POINT => self.reparse(owner, attr)?,
                _ => None,
            };
            if let Some(fact) = fact {
                self.fact(owner, fact);
            }
        }
        Some(())
    }

    fn name(&mut self, owner: Owner, attr: &[u8]) -> Option<()> {
        let value = resident_value(attr)?;
        let parent_ref = le64(value, 0)?;
        let chars = usize::from(*value.get(0x40)?);
        let namespace = *value.get(0x41)?;
        let raw = value.get(0x42..0x42 + chars * 2)?;
        let parent = parent_ref & RECORD_MASK;
        // The root lists itself as "." in its own directory; no other record
        // is its own parent.
        if namespace == NAMESPACE_DOS || parent == u64::from(owner.id) {
            return Some(());
        }
        let text_off = u32::try_from(self.text.len()).ok()?;
        let units = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]));
        // Lossy exactly the way `OsStr::to_string_lossy` is on Windows: one
        // U+FFFD per unpaired surrogate, so the names the walk would store
        // and the names stored here are the same strings.
        self.text.extend(
            char::decode_utf16(units).map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER)),
        );
        let text_len = u16::try_from(self.text.len() - text_off as usize).ok()?;
        self.names.push(Name {
            record: owner.id,
            parent: u32::try_from(parent).ok()?,
            text_off,
            seq: owner.seq,
            parent_seq: (parent_ref >> 48) as u16,
            text_len,
        });
        Some(())
    }

    /// A reparse point's fact: a name surrogate is a link. One whose data
    /// lives outside the record is read after the table (`resolve_reparse`).
    fn reparse(&mut self, owner: Owner, attr: &[u8]) -> Option<Option<Fact>> {
        if attr[8] != 0 {
            let lowest = le64(attr, 0x10)?;
            let runs = decode_runs(attr.get(usize::from(le16(attr, 0x20)?)..)?, lowest)?;
            let length = match lowest {
                0 => Some(le64(attr, 0x30)?),
                _ => None,
            };
            self.reparse.push(OutOfLineReparse {
                owner,
                runs,
                length,
            });
            return Some(None);
        }
        let tag = le32(resident_value(attr)?, 0)?;
        Some((tag & REPARSE_NAME_SURROGATE != 0).then_some(Fact::Surrogate))
    }

    /// Apply a fact now if it comes from the base record, or keep it for
    /// after the read if it comes from an extension.
    fn fact(&mut self, owner: Owner, fact: Fact) {
        if owner.is_base {
            apply(&mut self.slots[owner.id as usize], fact);
            return;
        }
        self.defer(owner, fact);
    }

    fn defer(&mut self, owner: Owner, fact: Fact) {
        self.deferred.push(Deferred {
            record: owner.id,
            seq: owner.seq,
            fact,
        });
    }

    /// Settle what was deferred and index the names by directory.
    fn finish(mut self) -> Table {
        for deferred in std::mem::take(&mut self.deferred) {
            let slot = &mut self.slots[deferred.record as usize];
            if slot.is(deferred.seq) {
                apply(slot, deferred.fact);
            }
        }

        let slots = &mut self.slots;
        // A name counts only if its record is still the one it was found in,
        // and lists only under a parent that is an in-use directory with the
        // sequence number the name expects — anything else is a leftover of
        // a file deleted or moved while the table was being read.
        self.names
            .retain(|name| slots[name.record as usize].is(name.seq));
        for name in &self.names {
            let slot = &mut slots[name.record as usize];
            slot.links = slot.links.saturating_add(1);
        }
        self.names.retain(|name| {
            u64::from(name.record) >= FIRST_USER_RECORD
                && slots
                    .get(name.parent as usize)
                    .is_some_and(|parent| parent.is(name.parent_seq) && parent.is_directory())
        });

        // Group by parent with a counting sort: the prefix sums are the
        // index `children` needs anyway, and scattering in table order keeps
        // each directory's entries in the order they were read, so two reads
        // of one table list alike.
        let mut first = vec![0u32; slots.len() + 1];
        for name in &self.names {
            first[name.parent as usize + 1] += 1;
        }
        for i in 1..first.len() {
            first[i] += first[i - 1];
        }
        let mut next = first.clone();
        let mut grouped = self.names.clone();
        for name in &self.names {
            let at = &mut next[name.parent as usize];
            grouped[*at as usize] = *name;
            *at += 1;
        }
        Table {
            slots: self.slots,
            names: grouped,
            first,
            text: self.text,
            bad_records: self.bad_records,
            bad_count: self.bad_count,
        }
    }
}

/// The fact an unnamed or WOF `$DATA` instance carries, if it is the
/// instance that carries sizes. `None` for a malformed header.
fn data_fact(attr: &[u8]) -> Option<Option<Fact>> {
    let name = attribute_name(attr)?;
    let fact = if name.is_empty() {
        stream_sizes(attr).map(|(size, alloc)| Fact::Data { size, alloc })
    } else if name == NAME_WOF {
        stream_sizes(attr).map(|(_, alloc)| Fact::Alloc(alloc))
    } else {
        None
    };
    Some(fact)
}

fn apply(slot: &mut Slot, fact: Fact) {
    match fact {
        Fact::Data { size, alloc } => {
            slot.size = size;
            slot.alloc += alloc;
        }
        Fact::Alloc(alloc) => slot.alloc += alloc,
        Fact::Surrogate => slot.flags |= SLOT_SURROGATE,
    }
}

/// A non-resident attribute's data as a byte stream over its runs: `$MFT`'s
/// own, an attribute list's, a reparse point's.
struct Stream<'a, R> {
    volume: &'a mut R,
    runs: Vec<Run>,
    cluster: u64,
}

impl<'a, R: Read + Seek> Stream<'a, R> {
    fn new(volume: &'a mut R, runs: Vec<Run>, geometry: Geometry) -> Self {
        Stream {
            volume,
            runs,
            cluster: geometry.bytes_per_cluster,
        }
    }

    /// Fill `buf` from virtual offset `at`. A record may straddle two runs
    /// when clusters are smaller than records, so this walks runs rather than
    /// assuming a record lives in one.
    fn read(&mut self, mut at: u64, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let vcn = at / self.cluster;
            let run = self
                .runs
                .iter()
                .find(|run| vcn >= run.vcn && vcn < run.vcn + run.len)
                .copied()
                .ok_or_else(|| invalid("read past the runs of a stream"))?;
            let run_end = (run.vcn + run.len) * self.cluster;
            let take = usize::try_from((run_end - at).min(buf.len() as u64)).unwrap_or(buf.len());
            let (head, rest) = buf.split_at_mut(take);
            match run.lcn {
                Some(lcn) => {
                    let disk = lcn * self.cluster + (at - run.vcn * self.cluster);
                    self.volume.seek(SeekFrom::Start(disk))?;
                    self.volume.read_exact(head)?;
                }
                None => head.fill(0),
            }
            at += take as u64;
            buf = rest;
        }
        Ok(())
    }
}

/// Read the table of the volume `volume` holds.
///
/// `progress(done, total)` is called once with `done == 0` before the first
/// record and then after every chunk with records read so far; returning
/// `false` stops the read with `ErrorKind::Interrupted` (invariant 5).
/// `total` is known from the start, which is what lets a caller show a share
/// rather than a bare count.
///
/// Any structural surprise — a boot sector that is not NTFS, a `$MFT` record
/// that will not parse, runs that do not cover the table — is an error, and
/// the caller's answer to an error is the ordinary walk. Records that cannot
/// be read are not errors of the read: they are counted in
/// [`Table::bad_count`] for the caller to report, the way the walk reports a
/// directory it could not open.
pub(crate) fn read_table<R: Read + Seek>(
    volume: &mut R,
    geometry: Geometry,
    mut progress: impl FnMut(u64, u64) -> bool,
) -> io::Result<Table> {
    let record_size = geometry.bytes_per_record as usize;
    let (mut stream, records) = mft_stream(volume, geometry)?;
    let cancelled = || io::Error::new(io::ErrorKind::Interrupted, "scan cancelled");
    if !progress(0, records) {
        return Err(cancelled());
    }

    let mut builder = Builder::with_records(records as usize);
    // Aligned for the sake of a Windows volume handle, which reads into
    // sector-aligned memory without a bounce copy (`volume.rs`).
    let chunk_records = (CHUNK_BYTES / record_size).max(1);
    let mut storage = vec![0u8; chunk_records * record_size + 4096];
    let align = storage.as_ptr().align_offset(4096).min(4096);
    let mut retry = vec![0u8; record_size];

    let mut next = 0u64;
    while next < records {
        let count = (records - next).min(chunk_records as u64) as usize;
        let chunk = &mut storage[align..align + count * record_size];
        stream.read(next * record_size as u64, chunk)?;
        for (i, record) in chunk.chunks_exact_mut(record_size).enumerate() {
            let number = next + i as u64;
            match builder.record(number, record) {
                Parsed::Done => {}
                Parsed::Bad => builder.bad(number),
                // Read once more, alone: a record caught mid-write on a live
                // volume reads whole a moment later.
                Parsed::Torn => {
                    stream.read(number * record_size as u64, &mut retry)?;
                    if !matches!(builder.record(number, &mut retry), Parsed::Done) {
                        builder.bad(number);
                    }
                }
            }
        }
        next += count as u64;
        if !progress(next, records) {
            return Err(cancelled());
        }
    }

    resolve_reparse(&mut builder, stream.volume, geometry);
    Ok(builder.finish())
}

/// Read the reparse points that live outside their records, for their tags.
///
/// The tag is the first four bytes of the data, read through the whole run
/// list the record's instances add up to — not the first run of whichever
/// instance came first, which need not start the data.
///
/// A tag that cannot be read — a read error, runs that do not reach offset
/// 0, data too short to hold one, a zero tag (what a hole reads as; no tag is
/// 0) — makes the record a bad record, counted and sampled like any other
/// (invariant 7), and never fails the table. Its entry stays, as a name
/// surrogate: listed, not descended into. Taking an unknown reparse point for
/// a directory would enter a junction and count whatever it points at twice.
fn resolve_reparse<R: Read + Seek>(builder: &mut Builder, volume: &mut R, geometry: Geometry) {
    let mut pending = std::mem::take(&mut builder.reparse);
    // Stable, so the bad records come out in table order.
    pending.sort_by_key(|item| (item.owner.id, item.owner.seq));
    for group in pending.chunk_by(|a, b| (a.owner.id, a.owner.seq) == (b.owner.id, b.owner.seq)) {
        let owner = group[0].owner;
        let length = group.iter().find_map(|item| item.length);
        let runs = group.iter().flat_map(|item| item.runs.iter().copied());
        match reparse_tag(volume, runs.collect(), length, geometry) {
            Some(tag) if tag & REPARSE_NAME_SURROGATE == 0 => {}
            Some(_) => builder.defer(owner, Fact::Surrogate),
            None => {
                // A base record already dropped as bad, or reused since its
                // extension was written, is not a second bad record.
                if builder.slots[owner.id as usize].is(owner.seq) {
                    builder.bad(u64::from(owner.id));
                }
                builder.defer(owner, Fact::Surrogate);
            }
        }
    }
}

/// The tag of a reparse point whose data lives in `runs`, or `None` where
/// there is none to be read.
fn reparse_tag<R: Read + Seek>(
    volume: &mut R,
    runs: Vec<Run>,
    length: Option<u64>,
    geometry: Geometry,
) -> Option<u32> {
    if length? < 4 {
        return None;
    }
    let mut tag = [0u8; 4];
    Stream::new(volume, runs, geometry).read(0, &mut tag).ok()?;
    let tag = u32::from_le_bytes(tag);
    (tag != 0).then_some(tag)
}

/// `$MFT`'s own data as a stream, and how many records it holds.
///
/// Record 0 describes the table, and on a volume whose table has grown in
/// many pieces its run list continues in extension records — found through
/// record 0's `$ATTRIBUTE_LIST`, and read through the runs known so far,
/// since NTFS keeps those extensions in the table's first extent.
fn mft_stream<R: Read + Seek>(
    volume: &mut R,
    geometry: Geometry,
) -> io::Result<(Stream<'_, R>, u64)> {
    let mut record = vec![0u8; geometry.bytes_per_record as usize];
    volume.seek(SeekFrom::Start(
        geometry.mft_lcn * geometry.bytes_per_cluster,
    ))?;
    volume.read_exact(&mut record)?;
    let mut sizes = None;
    let mut list = None;
    let runs = mft_record_runs(&mut record, &mut sizes, Some(&mut list))?;
    let list = match list {
        Some(attr) => attribute_list_bytes(volume, geometry, &attr)?,
        None => Vec::new(),
    };

    let mut stream = Stream::new(volume, runs, geometry);
    let mut pending = extension_records(&list);
    pending.retain(|&number| number != 0);
    pending.sort_unstable();
    pending.dedup();
    for number in pending {
        stream.read(number * geometry.bytes_per_record, &mut record)?;
        let more = mft_record_runs(&mut record, &mut sizes, None)?;
        stream.runs.extend(more);
    }

    let (data, initialized) = sizes.ok_or_else(|| invalid("$MFT has no data sizes"))?;
    stream.runs.sort_by_key(|run| run.vcn);
    let mapped = stream.runs.iter().map(|run| run.len).sum::<u64>() * geometry.bytes_per_cluster;
    let length = u64::min(data, initialized);
    if mapped < length {
        return Err(invalid("$MFT's runs do not cover its length"));
    }
    let records = length / geometry.bytes_per_record;
    // Names refer to records with 32-bit numbers here; a table past that
    // would be 4 TiB of records, and is refused rather than truncated.
    if records > u64::from(u32::MAX) {
        return Err(invalid("$MFT is larger than this reader handles"));
    }
    Ok((stream, records))
}

/// The runs of the unnamed `$DATA` instances in one of `$MFT`'s own records
/// (record 0 or an extension of it), noting the sizes if one carries them
/// and, when asked, the `$ATTRIBUTE_LIST`.
fn mft_record_runs(
    record: &mut [u8],
    sizes: &mut Option<(u64, u64)>,
    mut list: Option<&mut Option<Vec<u8>>>,
) -> io::Result<Vec<Run>> {
    let malformed = || invalid("a record of $MFT itself is malformed");
    if &record[0..4] != b"FILE" || !apply_fixups(record) {
        return Err(malformed());
    }
    let mut runs = Vec::new();
    for attr in attributes(record).ok_or_else(malformed)? {
        let attr = attr.ok_or_else(malformed)?;
        let kind = le32(attr, 0).ok_or_else(malformed)?;
        if kind == ATTR_DATA && attr[9] == 0 {
            data_runs(attr, &mut runs, sizes)?;
        } else if kind == ATTR_ATTRIBUTE_LIST {
            if let Some(list) = list.as_deref_mut() {
                *list = Some(attr.to_vec());
            }
        }
    }
    Ok(runs)
}

/// One instance of `$MFT`'s `$DATA`: its runs, and the sizes if it carries
/// them.
fn data_runs(attr: &[u8], runs: &mut Vec<Run>, sizes: &mut Option<(u64, u64)>) -> io::Result<()> {
    let malformed = || invalid("$MFT's $DATA is malformed");
    if attr[8] == 0 {
        return Err(invalid("$MFT's $DATA is resident"));
    }
    let lowest = le64(attr, 0x10).ok_or_else(malformed)?;
    if lowest == 0 {
        *sizes = Some((
            le64(attr, 0x30).ok_or_else(malformed)?,
            le64(attr, 0x38).ok_or_else(malformed)?,
        ));
    }
    let at = usize::from(le16(attr, 0x20).ok_or_else(malformed)?);
    let decoded =
        decode_runs(attr.get(at..).ok_or_else(malformed)?, lowest).ok_or_else(malformed)?;
    runs.extend(decoded);
    Ok(())
}

/// The bytes of an `$ATTRIBUTE_LIST`, wherever they live.
fn attribute_list_bytes<R: Read + Seek>(
    volume: &mut R,
    geometry: Geometry,
    attr: &[u8],
) -> io::Result<Vec<u8>> {
    let malformed = || invalid("$MFT's $ATTRIBUTE_LIST is malformed");
    if attr[8] == 0 {
        return Ok(resident_value(attr).ok_or_else(malformed)?.to_vec());
    }
    let length = le64(attr, 0x30).ok_or_else(malformed)?;
    let at = usize::from(le16(attr, 0x20).ok_or_else(malformed)?);
    let runs = decode_runs(attr.get(at..).ok_or_else(malformed)?, 0).ok_or_else(malformed)?;
    let length = usize::try_from(length).map_err(|_| malformed())?;
    if length > 1 << 20 {
        return Err(malformed());
    }
    let mut bytes = vec![0u8; length];
    Stream::new(volume, runs, geometry).read(0, &mut bytes)?;
    Ok(bytes)
}

/// Record numbers holding an unnamed `$DATA` instance, from an attribute
/// list's entries.
fn extension_records(list: &[u8]) -> Vec<u64> {
    let mut found = Vec::new();
    let mut at = 0usize;
    while let (Some(kind), Some(len)) = (le32(list, at), le16(list, at + 4)) {
        let len = usize::from(len);
        if len < 0x1A {
            break;
        }
        let name_len = list.get(at + 6).copied().unwrap_or(1);
        if kind == ATTR_DATA && name_len == 0 {
            if let Some(reference) = le64(list, at + 0x10) {
                found.push(reference & RECORD_MASK);
            }
        }
        at += len;
    }
    found
}

/// A record's attributes, each as its own slice. `None` for a header that
/// does not say where they are; an item of `None` for a chain that breaks
/// partway, after which the iterator ends.
fn attributes(record: &[u8]) -> Option<Attributes<'_>> {
    Some(Attributes {
        record,
        at: usize::from(le16(record, 0x14)?),
        used: (le32(record, 0x18)? as usize).min(record.len()),
    })
}

struct Attributes<'a> {
    record: &'a [u8],
    at: usize,
    used: usize,
}

impl<'a> Iterator for Attributes<'a> {
    type Item = Option<&'a [u8]>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.at + 8 > self.used {
            return None;
        }
        let at = self.at;
        if le32(self.record, at)? == ATTR_END {
            return None;
        }
        let len = le32(self.record, at + 4)? as usize;
        // Shorter than the smallest header, or running past the bytes in
        // use: nothing after this point can be trusted.
        if len < 0x18 || at + len > self.used {
            self.at = self.used;
            return Some(None);
        }
        self.at += len;
        Some(Some(&self.record[at..at + len]))
    }
}

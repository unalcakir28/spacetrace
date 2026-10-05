//! The two files of a macOS installer receipt, as pure functions over bytes.
//!
//! An installer package leaves `<id>.bom` and `<id>.plist` behind. The BOM is
//! the file list — every path the payload laid down, relative to where it was
//! installed. The plist says where that was (`InstallPrefixPath`) and under
//! what name `pkgutil` knows the package (`PackageIdentifier`).
//!
//! **Read here rather than through `pkgutil`**, measured on a macOS 27 host
//! with 147 receipts, 942,958 paths and 242 MB of BOM: `pkgutil --files` once
//! per package took 7.7 s, one process each, and parsing the same files here,
//! every path spelled out, 0.09 s. The answers are the same, path for path —
//! `every_receipt_here_lists_what_pkgutil_lists` below checks it against
//! `pkgutil` on whatever Mac runs it. Neither format is documented; both are
//! stable and widely read (`lsbom` ships with the system, and every plist on
//! disk is one of the two encodings below).
//!
//! Nothing here touches the disk, so it is tested everywhere, from a BOM made
//! by `mkbom` and committed under `testdata/`. Only macOS calls it at run
//! time; the loader compiles the call everywhere, so nothing here is dead.

use std::borrow::Cow;
use std::collections::HashMap;

/// What [`bom_paths`] hands the paths to, and asks before going into a folder.
pub trait Visit {
    /// Whether anything below `folder` can matter. Asked once for each folder
    /// that holds paths, parents first, and never for one below a folder that
    /// said no: a receipt installed at `/` costs a scan of `/usr/local` one
    /// question about `Applications`, not 140,000 paths inside Xcode.
    fn descend(&mut self, folder: &str) -> bool;
    /// One path, relative to the install location. Folders are paths too.
    fn path(&mut self, path: &str);
}

/// What became of a receipt's paths: handed out, or skipped because they lie
/// below a folder the visitor declined. Counted in the one loop over the one
/// set of paths, so together they are every path the receipt holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listing {
    pub handed: u64,
    pub skipped: u64,
}

/// The longest path a receipt may name, in bytes: macOS's `MAXPATHLEN`, which
/// no path on disk exceeds. The BOM stores each path as its folder's id and
/// its own name, so a chain of folders costs the file a few bytes a level and
/// every path below it the chain's whole length; without a bound a crafted
/// receipt would be spelled out quadratically.
const MAX_PATH_BYTES: u32 = 1024;

/// One path as the BOM stores it: the id of the folder it is in, and where
/// in the file its name is. Sixteen bytes, because Xcode's receipt holds
/// 142,000 of them and the whole list is read before anything is handed out.
struct Entry {
    id: u32,
    parent: u32,
    name_at: u32,
    name_len: u32,
}

impl Entry {
    fn name<'b>(&self, bytes: &'b [u8]) -> &'b [u8] {
        let at = self.name_at as usize;
        &bytes[at..at + self.name_len as usize]
    }
}

/// The block store a BOM is: a table of `(offset, length)` pairs, every
/// structure named by its index into it. Every read is bounds-checked — a
/// truncated receipt is an error, never a panic.
struct Store<'a> {
    bytes: &'a [u8],
    table: &'a [u8],
}

impl<'a> Store<'a> {
    fn open(bytes: &'a [u8]) -> Result<Store<'a>, String> {
        if bytes.get(..8) != Some(b"BOMStore") {
            return Err("not a BOM file".into());
        }
        // The header: magic, version, block count, then where the block
        // table and the named variables are.
        let index = be_uint(bytes, 16, 4).ok_or("header cut off")? as usize;
        let count = be_uint(bytes, index, 4).ok_or("block table outside the file")? as usize;
        let table = count
            .checked_mul(8)
            .and_then(|len| bytes.get(index + 4..index + 4 + len))
            .ok_or("block table runs past the end of the file")?;
        Ok(Store { bytes, table })
    }

    fn blocks(&self) -> usize {
        self.table.len() / 8
    }

    /// Where block `id` starts in the file, and the block itself.
    fn block_at(&self, id: u32) -> Result<(usize, &'a [u8]), String> {
        let at = id as usize * 8;
        let (Some(offset), Some(len)) =
            (be_uint(self.table, at, 4), be_uint(self.table, at + 4, 4))
        else {
            return Err(format!("block {id} is not in the table"));
        };
        let (offset, len) = (offset as usize, len as usize);
        let block = self
            .bytes
            .get(offset..offset.saturating_add(len))
            .filter(|_| id != 0)
            .ok_or_else(|| format!("block {id} lies outside the file"))?;
        Ok((offset, block))
    }

    fn block(&self, id: u64) -> Result<&'a [u8], String> {
        let id = u32::try_from(id).map_err(|_| format!("block {id} is not in the table"))?;
        self.block_at(id).map(|(_, block)| block)
    }

    /// The block the `Paths` variable points at: the root of the path tree.
    fn paths(&self) -> Result<&'a [u8], String> {
        let vars_at = be_uint(self.bytes, 24, 4).ok_or("header cut off")? as usize;
        let count = be_uint(self.bytes, vars_at, 4).ok_or("variables outside the file")?;
        let mut at = vars_at + 4;
        for _ in 0..count {
            let id = be_uint(self.bytes, at, 4).ok_or("variables cut off")?;
            let len = *self.bytes.get(at + 4).ok_or("variables cut off")? as usize;
            let name = self
                .bytes
                .get(at + 5..at + 5 + len)
                .ok_or("variables cut off")?;
            if name == b"Paths" {
                return self.block(id);
            }
            at += 5 + len;
        }
        Err("no `Paths` in it".into())
    }
}

/// Every path a receipt's BOM lists, relative to where the package was
/// installed and spelled as `pkgutil --files` spells it: no leading `./`, and
/// the install location itself (`.`) left out.
///
/// **All or nothing.** The whole file list is read and checked — every block
/// inside the file, every parent present, no loop — before the first path is
/// handed out, so a damaged receipt credits nothing rather than half its files.
///
/// **Only what can matter is spelled out.** Which folders can is the
/// visitor's call ([`Visit::descend`]), not a comparison of names here: a
/// receipt may name a folder through a symlink (`etc` for `/private/etc`), in
/// other capitals than the disk, or by its firmlinked name, and the loader's
/// resolver is what knows. Below a folder it declined, nothing is built.
///
/// Names that are not UTF-8 are converted the way the tree's names are, so the
/// two still compare equal.
pub fn bom_paths(bytes: &[u8], visit: &mut impl Visit) -> Result<Listing, String> {
    let entries = read_entries(bytes)?;
    let by_id = index_ids(&entries)?;
    let top = check_chains(bytes, &entries, &by_id)?;

    let mut walk = Walk {
        bytes,
        entries: &entries,
        by_id: &by_id,
        open: vec![Open::Unknown; entries.len()],
        folders: HashMap::new(),
        pending: Vec::new(),
    };
    let mut path = String::new();
    let (mut handed, mut skipped) = (0, 0);
    for (at, entry) in entries.iter().enumerate() {
        // The install location itself is not one of the paths.
        if top[at] != Top::Payload || entry.parent == 0 {
            continue;
        }
        let parent = by_id[entry.parent as usize] as usize;
        let Some(folder) = walk.folder(parent, visit) else {
            skipped += 1;
            continue;
        };
        path.clear();
        path.push_str(folder);
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(&String::from_utf8_lossy(entry.name(bytes)));
        visit.path(&path);
        handed += 1;
    }
    Ok(Listing { handed, skipped })
}

/// Every record of the path tree, read off the chain of leaves.
fn read_entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let store = Store::open(bytes)?;
    let tree = store.paths()?;
    if tree.get(..4) != Some(b"tree") {
        return Err("`Paths` is not a tree".into());
    }
    let mut node = store.block(be_uint(tree, 8, 4).ok_or("`Paths` cut off")?)?;
    let promised = be_uint(tree, 16, 4).ok_or("`Paths` cut off")? as usize;

    // Down the first child of every branch to the leftmost leaf, then along
    // the chain of leaves. Not by recursing through the branches: a branch
    // of N entries has N + 1 leaves below it, and the last is reachable only
    // through its neighbour (measured on `mkbom` output and on real receipts).
    // The depth of a B-tree over even a million paths is a handful, so a long
    // descent is a loop in a damaged file.
    let mut depth = 0;
    while be_uint(node, 0, 2).ok_or("a tree node is cut off")? == 0 {
        depth += 1;
        if depth > 64 {
            return Err("the path tree does not end".into());
        }
        node = store.block(be_uint(node, 12, 4).ok_or("a tree node is cut off")?)?;
    }

    // Every path takes an eight-byte slot in a leaf, so a count the file has
    // no room for is damage; and the chain is stopped the moment it hands out
    // more than the count, so a crafted one that keeps coming back to the
    // same leaves costs at most the count and one visit per block.
    if promised > bytes.len() / 8 {
        return Err(format!(
            "the path tree says it holds {promised} paths, more than the file has room for"
        ));
    }
    let mut entries = Vec::with_capacity(promised);
    let mut leaves = 0;
    loop {
        leaves += 1;
        if leaves > store.blocks() {
            return Err("the leaves of the path tree form a loop".into());
        }
        let count = be_uint(node, 2, 2).ok_or("a tree leaf is cut off")?;
        for i in 0..count as usize {
            let at = 12 + i * 8;
            let (Some(info), Some(file)) = (be_uint(node, at, 4), be_uint(node, at + 4, 4)) else {
                return Err("a tree leaf is cut off".into());
            };
            let info = store.block(info)?;
            let (file_at, file) = store.block_at(file as u32)?;
            let (Some(id), Some(parent)) = (be_uint(info, 0, 4), be_uint(file, 0, 4)) else {
                return Err("a path record is cut off".into());
            };
            let name = &file[4..];
            let name_len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            if entries.len() == promised {
                return Err(format!(
                    "the path tree holds more than the {promised} paths it says"
                ));
            }
            entries.push(Entry {
                id: id as u32,
                parent: parent as u32,
                name_at: u32::try_from(file_at + 4).map_err(|_| "the file is too large")?,
                name_len: name_len as u32,
            });
        }
        let forward = be_uint(node, 4, 4).ok_or("a tree leaf is cut off")?;
        if forward == 0 {
            break;
        }
        node = store.block(forward)?;
    }
    // The tree records how many paths it holds. A chain of leaves cut short
    // by one damaged link reads cleanly and silently loses the rest, so the
    // count is what tells the two apart.
    if entries.len() != promised {
        return Err(format!(
            "the path tree says it holds {promised} paths and {} were found",
            entries.len()
        ));
    }
    Ok(entries)
}

/// Id → position in `entries`, as a table: path ids are numbered from 1 with
/// no gaps on every receipt measured, so four bytes a path instead of a hash
/// map's twenty-odd. An id far beyond the count is a damaged file, not a
/// reason to allocate gigabytes.
fn index_ids(entries: &[Entry]) -> Result<Vec<u32>, String> {
    let highest = entries.iter().map(|e| e.id as usize).max().unwrap_or(0);
    if highest > entries.len().saturating_mul(16) + 1024 {
        return Err(format!(
            "a path id ({highest}) is far beyond the number of paths ({})",
            entries.len()
        ));
    }
    let mut by_id = vec![u32::MAX; highest + 1];
    for (at, entry) in entries.iter().enumerate() {
        if entry.id == 0 {
            return Err("a path has the id 0, which means \"no folder\"".into());
        }
        let slot = &mut by_id[entry.id as usize];
        if *slot != u32::MAX {
            return Err(format!("two paths share the id {}", entry.id));
        }
        *slot = at as u32;
    }
    Ok(by_id)
}

#[derive(Clone, Copy, PartialEq)]
enum Top {
    Unknown,
    Payload,
    Outside,
}

/// Where each chain of folders ends — and that every one ends, without a
/// loop: checked here, on integers, so nothing is handed out from a file
/// that fails. Only what lies below the top-level `.`, the install location
/// itself, is the payload. The system's data template also records `..`, a
/// file inside it and `.TemporaryItems` at the top level; neither `lsbom`
/// nor `pkgutil` lists them, and neither does this (measured).
///
/// Each payload path's length is worked out here too, from its folder's, and
/// one longer than [`MAX_PATH_BYTES`] refuses the receipt.
fn check_chains(bytes: &[u8], entries: &[Entry], by_id: &[u32]) -> Result<Vec<Top>, String> {
    let mut top = vec![Top::Unknown; entries.len()];
    let mut len = vec![0u32; entries.len()];
    let mut chain = Vec::new();
    for start in 0..entries.len() {
        chain.clear();
        let mut at = start;
        let ends = loop {
            if top[at] != Top::Unknown {
                break top[at];
            }
            chain.push(at);
            if chain.len() > entries.len() {
                return Err("the parents of a path form a loop".into());
            }
            let parent = entries[at].parent;
            if parent == 0 {
                break match entries[at].name(bytes) {
                    b"." => Top::Payload,
                    _ => Top::Outside,
                };
            }
            at = match by_id.get(parent as usize) {
                Some(&found) if found != u32::MAX => found as usize,
                _ => return Err(format!("a path's folder (id {parent}) is not in the file")),
            };
        };
        // Topmost first, so each folder's length is known before its paths'.
        for &done in chain.iter().rev() {
            top[done] = ends;
            let entry = &entries[done];
            if entry.parent == 0 {
                continue;
            }
            let above = by_id[entry.parent as usize] as usize;
            let separator = u32::from(entries[above].parent != 0);
            len[done] = len[above]
                .saturating_add(separator)
                .saturating_add(entry.name_len);
            if ends == Top::Payload && len[done] > MAX_PATH_BYTES {
                return Err(format!(
                    "a path is longer than the {MAX_PATH_BYTES} bytes macOS allows"
                ));
            }
        }
    }
    Ok(top)
}

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Unknown,
    Yes,
    No,
}

/// The folders asked about so far: whether each was entered, and the path
/// of each one that was.
struct Walk<'e> {
    bytes: &'e [u8],
    entries: &'e [Entry],
    by_id: &'e [u32],
    open: Vec<Open>,
    folders: HashMap<u32, String>,
    pending: Vec<usize>,
}

impl Walk<'_> {
    /// The path of the folder at `at`, if it was entered: built and asked
    /// about on first use, parents first, so each folder's path is built once
    /// and none below a declined folder is built at all. The chain was
    /// checked by the caller and ends at `.`.
    fn folder(&mut self, at: usize, visit: &mut impl Visit) -> Option<&str> {
        self.pending.clear();
        let mut up = at;
        while self.open[up] == Open::Unknown {
            self.pending.push(up);
            let parent = self.entries[up].parent;
            if parent == 0 {
                break;
            }
            up = self.by_id[parent as usize] as usize;
        }
        while let Some(here) = self.pending.pop() {
            let entry = &self.entries[here];
            if entry.parent == 0 {
                // `.`: the install location, which the caller already chose.
                self.open[here] = Open::Yes;
                self.folders.insert(here as u32, String::new());
                continue;
            }
            let parent = self.by_id[entry.parent as usize] as usize;
            if self.open[parent] == Open::No {
                self.open[here] = Open::No;
                continue;
            }
            let name = String::from_utf8_lossy(entry.name(self.bytes));
            let path = match self.folders[&(parent as u32)].as_str() {
                "" => name.into_owned(),
                above => format!("{above}/{name}"),
            };
            let enter = visit.descend(&path);
            self.open[here] = if enter { Open::Yes } else { Open::No };
            if enter {
                self.folders.insert(here as u32, path);
            }
        }
        match self.open[at] {
            Open::Yes => self.folders.get(&(at as u32)).map(String::as_str),
            _ => None,
        }
    }
}

/// What a receipt's plist says that the loader needs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReceiptInfo {
    /// The name `pkgutil` lists the package under. Taken from here and not
    /// from the file name, as `pkgutil` does: a receipt renamed on disk is
    /// still listed under its identifier.
    pub id: Option<String>,
    /// Where the BOM's paths start, relative to the volume: `/`,
    /// `Applications`, `usr/local/share/dotnet`, or empty for the volume root.
    pub prefix: Option<String>,
}

impl ReceiptInfo {
    /// The field a top-level key fills, if it is one of the two.
    fn slot(&mut self, key: &[u8]) -> Option<&mut Option<String>> {
        match key {
            b"PackageIdentifier" => Some(&mut self.id),
            b"InstallPrefixPath" => Some(&mut self.prefix),
            _ => None,
        }
    }
}

/// The two keys of a receipt plist, in either encoding a plist is written in.
/// Receipts are binary (`bplist00`) almost always; the one written by the
/// system's data template is XML. Only the two values are decoded.
pub fn receipt_info(bytes: &[u8]) -> Result<ReceiptInfo, String> {
    match bytes.starts_with(b"bplist00") {
        true => binary_plist_info(bytes),
        false => xml_plist_info(bytes),
    }
}

fn binary_plist_info(bytes: &[u8]) -> Result<ReceiptInfo, String> {
    let bad = || "binary plist trailer is bad".to_string();
    let trailer_at = bytes
        .len()
        .checked_sub(32)
        .filter(|&at| at >= 8)
        .ok_or("binary plist cut off")?;
    let trailer = &bytes[trailer_at..];
    let offset_size = trailer[6] as usize;
    let ref_size = trailer[7] as usize;
    let objects = be_uint(trailer, 8, 8).ok_or_else(bad)?;
    let top = be_uint(trailer, 16, 8).ok_or_else(bad)?;
    let table = be_uint(trailer, 24, 8).ok_or_else(bad)?;
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) || top >= objects {
        return Err(bad());
    }
    let table_len = objects.checked_mul(offset_size as u64).ok_or_else(bad)?;
    if table.saturating_add(table_len) > trailer_at as u64 {
        return Err("binary plist offset table runs past the end".into());
    }
    let plist = BinaryPlist {
        bytes,
        offset_size,
        ref_size,
        objects,
        table: table as usize,
    };

    let (kind, count, body) = plist.header(plist.object(top)?)?;
    if kind != 0xD {
        return Err("the top of the plist is not a dictionary".into());
    }
    // `count` key references, then `count` value references. Checked before
    // any position is computed from them: the count is the file's word.
    let refs_end = count
        .checked_mul(2 * ref_size)
        .and_then(|len| body.checked_add(len));
    if refs_end.is_none_or(|end| end > plist.table) {
        return Err("the plist's dictionary runs past its objects".into());
    }
    let mut info = ReceiptInfo::default();
    for i in 0..count {
        let key = plist.reference(body + i * ref_size)?;
        // Keys are compared as they are stored; both wanted ones are ASCII,
        // which a binary plist always stores as such.
        let Some((0x5, key)) = plist.raw(key)? else {
            continue;
        };
        let Some(slot) = info.slot(key) else {
            continue;
        };
        let value = plist.reference(body + (count + i) * ref_size)?;
        *slot = plist.string(value)?;
    }
    Ok(info)
}

struct BinaryPlist<'a> {
    bytes: &'a [u8],
    offset_size: usize,
    ref_size: usize,
    objects: u64,
    table: usize,
}

impl<'a> BinaryPlist<'a> {
    /// Where object `id` starts.
    fn object(&self, id: u64) -> Result<usize, String> {
        if id >= self.objects {
            return Err(format!("plist object {id} does not exist"));
        }
        let at = self.table + id as usize * self.offset_size;
        let offset =
            be_uint(self.bytes, at, self.offset_size).ok_or("plist offset table cut off")?;
        match offset < self.table as u64 {
            true => Ok(offset as usize),
            false => Err(format!("plist object {id} lies past its own table")),
        }
    }

    fn reference(&self, at: usize) -> Result<u64, String> {
        be_uint(self.bytes, at, self.ref_size).ok_or_else(|| "a plist reference is cut off".into())
    }

    /// An object's type nibble, its length, and where its body starts. A
    /// length that does not fit in four bits follows as an integer object.
    fn header(&self, at: usize) -> Result<(u8, usize, usize), String> {
        let marker = *self.bytes.get(at).ok_or("a plist object is cut off")?;
        let (kind, short) = (marker >> 4, (marker & 0xF) as usize);
        if short != 0xF {
            return Ok((kind, short, at + 1));
        }
        let int = *self.bytes.get(at + 1).ok_or("a plist object is cut off")?;
        if int >> 4 != 0x1 || int & 0xF > 3 {
            return Err("a plist length is not an integer".into());
        }
        let width = 1usize << (int & 0xF);
        let len = be_uint(self.bytes, at + 2, width).ok_or("a plist object is cut off")?;
        Ok((kind, len as usize, at + 2 + width))
    }

    /// A string object's type and stored bytes, or `None` for anything else
    /// — a date, a nested dictionary — which the loader has no use for.
    fn raw(&self, id: u64) -> Result<Option<(u8, &'a [u8])>, String> {
        let (kind, len, body) = self.header(self.object(id)?)?;
        let units = match kind {
            0x5 | 0x7 => len,
            0x6 => len.checked_mul(2).ok_or("a plist string is too long")?,
            _ => return Ok(None),
        };
        let raw = self
            .bytes
            .get(body..body.saturating_add(units))
            .ok_or("a plist string is cut off")?;
        Ok(Some((kind, raw)))
    }

    /// Object `id` as text, or `None` when it is not a string.
    fn string(&self, id: u64) -> Result<Option<String>, String> {
        let Some((kind, raw)) = self.raw(id)? else {
            return Ok(None);
        };
        Ok(Some(match kind {
            0x6 => {
                let units: Vec<u16> = raw
                    .chunks_exact(2)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            }
            _ => String::from_utf8_lossy(raw).into_owned(),
        }))
    }
}

/// The two keys of an XML plist's top-level dictionary.
///
/// **A tokenizer, not a search for `<key>PackageIdentifier</key>`**: plists
/// are written with their keys sorted, so a nested dictionary under
/// `AdditionalInformation` comes before `PackageIdentifier`, and the system's
/// data template has one. A key of that name inside it would be found first
/// by a search; here it is at depth 2 and skipped. Only the two values are
/// unescaped.
fn xml_plist_info(bytes: &[u8]) -> Result<ReceiptInfo, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the plist is neither binary nor UTF-8")?;
    let start = text.find("<plist").ok_or("not a plist")?;
    let mut rest = &text[start..];
    let mut depth = 0usize;
    let mut info = ReceiptInfo::default();
    // The key just read at the top level, if it is one of the two.
    let mut wanted: Option<String> = None;
    while let Some(open) = rest.find('<') {
        let markup = &rest[open..];
        if let Some(after) = skip_markup(markup)? {
            rest = after;
            continue;
        }
        let close = markup.find('>').ok_or("an XML tag is not closed")?;
        let tag = &markup[1..close];
        rest = &markup[close + 1..];
        let name = tag
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");
        let empty = tag.ends_with('/');
        if name.starts_with('/') {
            if name == "/dict" || name == "/array" {
                depth = depth.saturating_sub(1);
                // The top-level dictionary closed: the rest is `</plist>`.
                if depth == 0 {
                    return Ok(info);
                }
            }
            continue;
        }
        if depth == 0 && name != "plist" && name != "dict" {
            return Err("the top of the plist is not a dictionary".into());
        }
        if depth == 0 && name == "dict" && empty {
            return Ok(info);
        }
        // Text is only read for a key or a string; anything else at the top
        // level — a date, a number, a nested container — is a value the
        // pending key does not want.
        let text = match (name, empty) {
            ("key" | "string", false) => {
                let (text, after) = element_text(rest, name)?;
                rest = after;
                Some(text)
            }
            ("string", true) => Some(String::new()),
            _ => None,
        };
        if depth == 1 {
            match (name, text) {
                ("key", Some(key)) => {
                    wanted = info.slot(key.as_bytes()).is_some().then_some(key);
                }
                ("string", Some(value)) => {
                    if let Some(slot) = wanted.take().and_then(|key| info.slot(key.as_bytes())) {
                        *slot = Some(value);
                    }
                }
                _ => wanted = None,
            }
        }
        if (name == "dict" || name == "array") && !empty {
            depth += 1;
        }
    }
    // Ended before the dictionary did: a receipt cut off mid-write.
    Err("the plist is cut off".into())
}

/// What follows `markup` when it is not an element — a comment, a processing
/// instruction, a CDATA section or the DOCTYPE — or `None` when it is one.
/// Each ends at its own terminator: a comment that holds a `>` does not end
/// there, and a key inside it is not a key. A DOCTYPE with an internal subset
/// would end early; Apple's has none.
fn skip_markup(markup: &str) -> Result<Option<&str>, String> {
    let ends = [
        ("<!--", "-->"),
        ("<![CDATA[", "]]>"),
        ("<?", "?>"),
        ("<!", ">"),
    ];
    let Some((start, end)) = ends
        .into_iter()
        .find(|(start, _)| markup.starts_with(start))
    else {
        return Ok(None);
    };
    let inside = &markup[start.len()..];
    let len = inside
        .find(end)
        .ok_or("an XML comment or declaration is not closed")?;
    Ok(Some(&inside[len + end.len()..]))
}

/// The text of the element `name`, whose start tag was just read, and what
/// follows its end tag: entities decoded, a CDATA section taken as it stands,
/// comments dropped. Any other markup there is not a plist.
fn element_text<'t>(mut rest: &'t str, name: &str) -> Result<(String, &'t str), String> {
    let mut text = String::new();
    loop {
        let open = rest.find('<').ok_or("an XML element is not closed")?;
        text.push_str(&unescape(&rest[..open]));
        let markup = &rest[open..];
        if let Some(cdata) = markup.strip_prefix("<![CDATA[") {
            let end = cdata.find("]]>").ok_or("a CDATA section is not closed")?;
            text.push_str(&cdata[..end]);
            rest = &cdata[end + 3..];
            continue;
        }
        if let Some(comment) = markup.strip_prefix("<!--") {
            let end = comment.find("-->").ok_or("an XML comment is not closed")?;
            rest = &comment[end + 3..];
            continue;
        }
        let after = markup
            .strip_prefix("</")
            .and_then(|m| m.strip_prefix(name))
            .and_then(|m| m.trim_start().strip_prefix('>'))
            .ok_or_else(|| format!("unexpected markup inside <{name}>"))?;
        return Ok((text, after));
    }
}

/// The longest entity, `&` and `;` included: `&#x10FFFF;`, or the decimal
/// one, with a byte to spare.
const MAX_ENTITY: usize = 12;

/// XML's five named entities and numeric references.
fn unescape(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // Only as far as an entity can reach, so a run of `&` costs one look
        // each, not a scan to the end.
        let semi = rest.bytes().take(MAX_ENTITY).position(|b| b == b';');
        let Some(semi) = semi else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded: Option<char> = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|hex| u32::from_str_radix(hex, 16))
                .or_else(|| entity.strip_prefix('#').map(|dec| dec.parse::<u32>()))
                .and_then(|n| n.ok())
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The big-endian unsigned integer of `width` bytes (one to eight) at `at`,
/// if the bytes are there.
fn be_uint(bytes: &[u8], at: usize, width: usize) -> Option<u64> {
    if !(1..=8).contains(&width) {
        return None;
    }
    let raw = bytes.get(at..at.checked_add(width)?)?;
    Some(raw.iter().fold(0u64, |n, &b| (n << 8) | b as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `testdata/receipt-fixture.bom` holds: the tree `fixture_tree`
    /// builds, as `mkbom` recorded it. Enough files in one folder that the
    /// path tree needs a branch and more than one leaf, a symlink, a bundle
    /// with spaces in its names, and a name that is not ASCII.
    fn fixture_expected() -> Vec<String> {
        let mut paths: Vec<String> = [
            "Tiny App.app",
            "Tiny App.app/Contents",
            "Tiny App.app/Contents/MacOS",
            "Tiny App.app/Contents/MacOS/Tiny App",
            "many",
            "usr",
            "usr/local",
            "usr/local/bin",
            "usr/local/bin/tool",
            "usr/local/bin/tool-link",
            "ünïcødé ✓",
        ]
        .map(String::from)
        .into();
        paths.extend((0..FIXTURE_MANY).map(|i| format!("many/f{i:03}")));
        paths.sort();
        paths
    }

    const FIXTURE_MANY: usize = 600;

    const FIXTURE_BOM: &[u8] = include_bytes!("testdata/receipt-fixture.bom");

    /// Takes every path, enters every folder unless told which to skip, and
    /// remembers what it was asked.
    #[derive(Default)]
    struct Collect {
        paths: Vec<String>,
        asked: Vec<String>,
        skip: Vec<&'static str>,
    }

    impl Visit for Collect {
        fn descend(&mut self, folder: &str) -> bool {
            self.asked.push(folder.to_string());
            !self.skip.contains(&folder)
        }
        fn path(&mut self, path: &str) {
            self.paths.push(path.to_string());
        }
    }

    fn parsed(bytes: &[u8]) -> Result<Vec<String>, String> {
        let mut all = Collect::default();
        let listing = bom_paths(bytes, &mut all)?;
        assert_eq!(listing.handed, all.paths.len() as u64);
        assert_eq!(listing.skipped, 0, "nothing was declined");
        all.paths.sort();
        Ok(all.paths)
    }

    /// The root of the fixture's path tree.
    fn root_block(store: &Store<'static>) -> &'static [u8] {
        let tree = store.paths().unwrap();
        store.block(be_uint(tree, 8, 4).unwrap()).unwrap()
    }

    /// The fixture's leftmost leaf.
    fn first_leaf(store: &Store<'static>) -> &'static [u8] {
        let root = root_block(store);
        match be_uint(root, 0, 2) {
            Some(0) => store.block(be_uint(root, 12, 4).unwrap()).unwrap(),
            _ => root,
        }
    }

    #[test]
    fn a_bom_lists_every_path_below_the_install_location() {
        assert_eq!(parsed(FIXTURE_BOM).unwrap(), fixture_expected());
    }

    /// The fixture is only a test of the tree walk if the tree has a branch
    /// and a leaf the branch does not point at — the one only the chain from
    /// its neighbour reaches.
    #[test]
    fn the_fixture_has_a_leaf_only_the_chain_reaches() {
        let store = Store::open(FIXTURE_BOM).unwrap();
        let root = root_block(&store);
        assert_eq!(be_uint(root, 0, 2), Some(0), "the root is a branch");
        let pointed = be_uint(root, 2, 2).unwrap();
        let mut leaf = first_leaf(&store);
        let mut leaves = 1;
        while be_uint(leaf, 4, 4).unwrap() != 0 {
            leaf = store.block(be_uint(leaf, 4, 4).unwrap()).unwrap();
            leaves += 1;
        }
        assert!(leaves > pointed, "{leaves} leaves, {pointed} pointed at");
    }

    /// A folder the visitor declines costs one question: nothing below it is
    /// asked about or handed out, and the paths it held are still counted.
    #[test]
    fn nothing_below_a_declined_folder_is_built() {
        let mut some = Collect {
            skip: vec!["many", "Tiny App.app/Contents"],
            ..Collect::default()
        };
        let listing = bom_paths(FIXTURE_BOM, &mut some).unwrap();

        assert_eq!(
            listing.handed + listing.skipped,
            fixture_expected().len() as u64
        );
        some.paths.sort();
        let expected: Vec<String> = fixture_expected()
            .into_iter()
            .filter(|p| !p.starts_with("many/") && !p.starts_with("Tiny App.app/Contents/"))
            .collect();
        assert_eq!(some.paths, expected, "the folders themselves are paths");
        assert_eq!(listing.handed, expected.len() as u64);
        some.asked.sort();
        assert_eq!(
            some.asked,
            [
                "Tiny App.app",
                "Tiny App.app/Contents",
                "many",
                "usr",
                "usr/local",
                "usr/local/bin"
            ],
            "asked once per folder that holds paths, never below a no"
        );
    }

    /// A damaged receipt is an error, never a panic and never half a list:
    /// truncations of a real BOM, and a byte overwritten at every offset of
    /// its header and structures, either read whole or hand out nothing.
    #[test]
    fn a_damaged_bom_is_an_error_and_hands_out_nothing() {
        let check = |bytes: &[u8]| -> bool {
            let mut all = Collect::default();
            match bom_paths(bytes, &mut all) {
                Ok(listing) => {
                    assert_eq!(listing.handed, all.paths.len() as u64);
                    true
                }
                Err(_) => {
                    assert!(
                        all.paths.is_empty() && all.asked.is_empty(),
                        "handed out, then failed"
                    );
                    false
                }
            }
        };
        let mut refused = 0;
        for len in (0..FIXTURE_BOM.len()).step_by(97) {
            refused += !check(&FIXTURE_BOM[..len]) as usize;
        }
        assert!(refused > 0, "no truncation was noticed");
        let mut bytes = FIXTURE_BOM.to_vec();
        for at in 0..bytes.len() {
            // Every byte of the header and the first blocks, a sample of
            // the rest.
            if at > 0x400 && at % 61 != 0 {
                continue;
            }
            for value in [0x00, 0xFF, 0x7F] {
                let was = bytes[at];
                bytes[at] = value;
                check(&bytes);
                bytes[at] = was;
            }
        }
        assert!(bom_paths(b"not a bom at all", &mut Collect::default()).is_err());
    }

    /// The fixture's record for the top-level `name`: its id, and where in
    /// the file its parent field and its id field are, so a test can rewire
    /// them.
    fn record(name: &[u8]) -> (u32, usize, usize) {
        let found = records()
            .into_iter()
            .find(|r| r.name == name && r.parent <= 1)
            .unwrap();
        (found.id, found.parent_at, found.id_at)
    }

    /// One record of the fixture, with where its fields are in the file.
    struct Record {
        name: Vec<u8>,
        id: u32,
        parent: u32,
        parent_at: usize,
        id_at: usize,
    }

    /// Every record of the fixture, in the order of its leaves.
    fn records() -> Vec<Record> {
        let store = Store::open(FIXTURE_BOM).unwrap();
        let mut leaf = first_leaf(&store);
        let mut all = Vec::new();
        loop {
            for i in 0..be_uint(leaf, 2, 2).unwrap() as usize {
                let (id_at, info) = store
                    .block_at(be_uint(leaf, 12 + i * 8, 4).unwrap() as u32)
                    .unwrap();
                let (parent_at, file) = store
                    .block_at(be_uint(leaf, 16 + i * 8, 4).unwrap() as u32)
                    .unwrap();
                let end = file.iter().skip(4).position(|&b| b == 0).unwrap() + 4;
                all.push(Record {
                    name: file[4..end].to_vec(),
                    id: be_uint(info, 0, 4).unwrap() as u32,
                    parent: be_uint(file, 0, 4).unwrap() as u32,
                    parent_at,
                    id_at,
                });
            }
            match be_uint(leaf, 4, 4).unwrap() {
                0 => return all,
                next => leaf = store.block(next).unwrap(),
            }
        }
    }

    /// A parent chain that loops: the top record, `.`, made its own parent.
    #[test]
    fn a_folder_loop_is_refused() {
        let (id, parent_at, _) = record(b".");
        let mut bytes = FIXTURE_BOM.to_vec();
        bytes[parent_at..parent_at + 4].copy_from_slice(&id.to_be_bytes());

        let mut all = Collect::default();
        let err = bom_paths(&bytes, &mut all).unwrap_err();
        assert!(err.contains("loop"), "{err}");
        assert!(all.paths.is_empty());
    }

    /// Two records with one id: which folder a path below them is in would
    /// depend on which was read last, so the receipt is refused instead.
    #[test]
    fn two_paths_with_one_id_are_refused() {
        let (id, _, _) = record(b".");
        let (_, _, info_at) = record(b"usr");
        let mut bytes = FIXTURE_BOM.to_vec();
        bytes[info_at..info_at + 4].copy_from_slice(&id.to_be_bytes());

        let mut all = Collect::default();
        let err = bom_paths(&bytes, &mut all).unwrap_err();
        assert!(err.contains("share the id"), "{err}");
        assert!(all.paths.is_empty());
    }

    /// A chain of leaves that comes back to its start would hand out the
    /// same records for as long as the block table allows — billions of
    /// entries from a crafted file. It is stopped as soon as it holds more
    /// than the tree says it does.
    #[test]
    fn a_chain_of_leaves_that_returns_to_its_start_is_stopped_at_the_count() {
        let store = Store::open(FIXTURE_BOM).unwrap();
        let root = root_block(&store);
        assert_eq!(be_uint(root, 0, 2), Some(0), "the root is a branch");
        let first = be_uint(root, 12, 4).unwrap() as u32;
        let mut id = first;
        let last_at = loop {
            let (at, leaf) = store.block_at(id).unwrap();
            assert_eq!(be_uint(leaf, 0, 2), Some(1), "the branch points at leaves");
            match be_uint(leaf, 4, 4).unwrap() as u32 {
                0 => break at,
                next => id = next,
            }
        };
        let mut bytes = FIXTURE_BOM.to_vec();
        bytes[last_at + 4..last_at + 8].copy_from_slice(&first.to_be_bytes());

        let mut all = Collect::default();
        let err = bom_paths(&bytes, &mut all).unwrap_err();
        assert!(err.contains("more than"), "{err}");
        assert!(all.paths.is_empty());
    }

    /// A BOM names each path by its folder's id, so a chain of folders costs
    /// a few bytes a level in the file and its whole length again in every
    /// path below it: a crafted chain makes the paths quadratic in the file.
    /// No path on macOS is longer than `MAXPATHLEN`, so a receipt naming one
    /// is damaged, and refused before anything is built. Here the 600
    /// `many/fNNN` files are made a chain of folders, 3,000 bytes deep.
    #[test]
    fn a_path_longer_than_macos_allows_is_refused() {
        let mut many: Vec<Record> = records()
            .into_iter()
            .filter(|r| r.name.len() == 4 && r.name[0] == b'f')
            .collect();
        assert_eq!(many.len(), FIXTURE_MANY);
        many.sort_by(|a, b| a.name.cmp(&b.name));
        let mut bytes = FIXTURE_BOM.to_vec();
        for pair in many.windows(2) {
            let at = pair[1].parent_at;
            bytes[at..at + 4].copy_from_slice(&pair[0].id.to_be_bytes());
        }

        let mut all = Collect::default();
        let err = bom_paths(&bytes, &mut all).unwrap_err();
        assert!(err.contains("longer"), "{err}");
        assert!(all.paths.is_empty() && all.asked.is_empty());
    }

    /// The data template's `..` and `.TemporaryItems` sit at the top level
    /// beside `.`, outside the install location; `pkgutil` does not list
    /// them or anything below them. Here `usr` is moved up beside `.`.
    #[test]
    fn a_top_level_entry_beside_the_install_location_is_not_listed() {
        let (_, parent_at, _) = record(b"usr");
        let mut bytes = FIXTURE_BOM.to_vec();
        bytes[parent_at..parent_at + 4].copy_from_slice(&0u32.to_be_bytes());

        let without: Vec<String> = fixture_expected()
            .into_iter()
            .filter(|path| !path.starts_with("usr"))
            .collect();
        assert_eq!(parsed(&bytes).unwrap(), without);
    }

    // ------------------------------------------------------------ plists

    /// `testdata/receipt-fixture.plist`, written by `plutil -convert binary1`
    /// from [`FIXTURE_PLIST_XML`]: the identifier is longer than fifteen
    /// characters, so its length is a separate integer; the prefix is not
    /// ASCII, so it is stored as UTF-16. Nested dictionaries holding the same
    /// two keys come first and last, so a reader that took their keys for the
    /// top level's — first match or last — would answer wrong.
    const FIXTURE_PLIST: &[u8] = include_bytes!("testdata/receipt-fixture.plist");

    const FIXTURE_PLIST_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AdditionalInformation</key>
	<dict>
		<key>InstallPrefixPath</key>
		<string>nested/first</string>
		<key>PackageIdentifier</key>
		<string>nested.first</string>
	</dict>
	<key>InstallDate</key>
	<date>2026-10-01T07:13:58Z</date>
	<key>InstallPrefixPath</key>
	<string>Applications/Ünïcødé &amp; Co</string>
	<key>InstallProcessName</key>
	<string>installer</string>
	<key>PackageIdentifier</key>
	<string>com.example.spacetrace.fixture</string>
	<key>PackageVersion</key>
	<string>1.0</string>
	<key>ZAdditionalInformation</key>
	<dict>
		<key>PackageIdentifier</key>
		<string>not.the.top.level</string>
		<key>Patterns</key>
		<array>
			<string>Library/**</string>
		</array>
	</dict>
</dict>
</plist>
"#;

    fn fixture_info() -> ReceiptInfo {
        ReceiptInfo {
            id: Some("com.example.spacetrace.fixture".into()),
            prefix: Some("Applications/Ünïcødé & Co".into()),
        }
    }

    #[test]
    fn a_binary_receipt_plist_gives_its_identifier_and_prefix() {
        assert_eq!(receipt_info(FIXTURE_PLIST).unwrap(), fixture_info());
    }

    #[test]
    fn an_xml_receipt_plist_gives_the_same_and_ignores_nested_keys() {
        assert_eq!(
            receipt_info(FIXTURE_PLIST_XML.as_bytes()).unwrap(),
            fixture_info()
        );
    }

    /// An empty prefix — seven receipts on the measuring machine — is the
    /// volume root, and an absent key is not an empty one.
    #[test]
    fn an_empty_prefix_is_kept_apart_from_a_missing_one() {
        let xml = "<plist><dict><key>InstallPrefixPath</key><string/>\
                   <key>PackageIdentifier</key><string>a</string></dict></plist>";
        let info = receipt_info(xml.as_bytes()).unwrap();
        assert_eq!(info.prefix.as_deref(), Some(""));
        assert_eq!(info.id.as_deref(), Some("a"));
        let xml = "<plist><dict><key>PackageIdentifier</key><string>a</string></dict></plist>";
        assert_eq!(receipt_info(xml.as_bytes()).unwrap().prefix, None);
    }

    #[test]
    fn a_damaged_plist_is_an_error_not_a_panic() {
        for len in 0..FIXTURE_PLIST.len() {
            let _ = receipt_info(&FIXTURE_PLIST[..len]);
        }
        let mut bytes = FIXTURE_PLIST.to_vec();
        for at in 0..bytes.len() {
            for value in [0x00, 0xFF, 0x0F, 0xDF, 0x5F, 0x6F] {
                let was = bytes[at];
                bytes[at] = value;
                let _ = receipt_info(&bytes);
                bytes[at] = was;
            }
        }
        assert!(receipt_info(b"bplist00").is_err());
        assert!(receipt_info(b"<plist><dict><key>a</key>").is_err());
        assert!(receipt_info(&[0xFF, 0xFE, 0x00]).is_err());
    }

    /// A dictionary claiming 2^63 entries: where its values start is then
    /// past the end of memory, and computing it overflowed before anything
    /// was checked.
    #[test]
    fn a_dictionary_larger_than_its_plist_is_refused() {
        let mut bytes = b"bplist00".to_vec();
        let dict_at = bytes.len();
        bytes.extend([0xDF, 0x13]);
        bytes.extend((1u64 << 63).to_be_bytes());
        bytes.extend([0x00, 0x01]);
        let key_at = bytes.len();
        bytes.extend([0x5F, 0x10, 17]);
        bytes.extend(b"PackageIdentifier");
        let table = bytes.len();
        bytes.extend([dict_at as u8, key_at as u8]);
        let mut trailer = [0u8; 32];
        trailer[6] = 1;
        trailer[7] = 2;
        trailer[8..16].copy_from_slice(&2u64.to_be_bytes());
        trailer[24..32].copy_from_slice(&(table as u64).to_be_bytes());
        bytes.extend(trailer);

        let err = receipt_info(&bytes).unwrap_err();
        assert!(err.contains("dictionary"), "{err}");
    }

    /// A comment, a processing instruction or a CDATA section ends at its own
    /// terminator, not the first `>`: a key inside one is not a key, and a
    /// CDATA section inside a string is that string's text as it stands.
    #[test]
    fn xml_comments_and_cdata_are_not_read_as_keys() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>InstallPrefixPath</key>
	<string><![CDATA[a<b&amp;]]>c<!-- d --></string>
	<!-- > <key>InstallPrefixPath</key><string>/evil</string> -->
	<key>PackageIdentifier</key>
	<string>good</string>
	<?pi > <key>PackageIdentifier</key><string>evil</string> ?>
</dict>
</plist>
"#;
        assert_eq!(
            receipt_info(xml.as_bytes()).unwrap(),
            ReceiptInfo {
                id: Some("good".into()),
                prefix: Some("a<b&amp;c".into()),
            }
        );
    }

    /// An entity is a few characters long, so the search for its `;` is too:
    /// a million `&` before one `;` took a scan to the end per `&`.
    #[test]
    fn unescaping_is_linear_in_the_text() {
        let text = format!("{};", "&".repeat(1_000_000));
        let started = std::time::Instant::now();
        assert_eq!(unescape(&text), text);
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn xml_entities_are_decoded() {
        assert_eq!(
            unescape("a &amp; b &lt;c&gt; &#233;&#x2713;"),
            "a & b <c> é✓"
        );
        assert_eq!(unescape("AT&T; &bogus; tail &"), "AT&T; &bogus; tail &");
    }

    // --------------------------------------------- against Apple's tools

    /// The tree the fixture was made from.
    #[cfg(target_os = "macos")]
    fn fixture_tree(dir: &std::path::Path) {
        for folder in ["Tiny App.app/Contents/MacOS", "many", "usr/local/bin"] {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
        }
        for path in fixture_expected() {
            let at = dir.join(&path);
            if at.exists() {
                continue;
            }
            match path.as_str() {
                "usr/local/bin/tool-link" => std::os::unix::fs::symlink("tool", &at).unwrap(),
                _ => std::fs::write(&at, path.as_bytes()).unwrap(),
            }
        }
    }

    /// A command's output, one line per path. Split on `\n` alone: `lines()`
    /// would also eat the `\r` that ends a real name, Python's `Icon\r`.
    #[cfg(target_os = "macos")]
    fn command_lines(out: std::process::Output) -> Vec<String> {
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout)
            .split_terminator('\n')
            .map(str::to_string)
            .collect()
    }

    /// `lsbom` as the oracle: Apple's own reader of the format, over a BOM
    /// made by Apple's own writer from the fixture's recipe — which also
    /// proves the committed fixture is still what the recipe gives. To
    /// regenerate it, keep the `fresh.bom` this writes.
    #[cfg(target_os = "macos")]
    #[test]
    fn mkbom_and_lsbom_agree_with_the_parser() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fixture_tree(&src);
        let bom = dir.path().join("fresh.bom");
        let made = Command::new("mkbom").arg(&src).arg(&bom).status().unwrap();
        assert!(made.success());
        let bytes = std::fs::read(&bom).unwrap();
        assert_eq!(parsed(&bytes).unwrap(), fixture_expected());

        let listed = Command::new("lsbom").arg("-s").arg(&bom).output().unwrap();
        let mut oracle: Vec<String> = command_lines(listed)
            .iter()
            .filter_map(|line| line.strip_prefix("./").map(str::to_string))
            .collect();
        oracle.sort();
        assert_eq!(oracle, fixture_expected());
    }

    /// `pkgutil` as the oracle, over every receipt this Mac holds: the same
    /// paths as `pkgutil --files`, for every package. Ignored because it
    /// reads the machine rather than a fixture and takes about 15 s of
    /// `pkgutil`; run it with
    /// `cargo test -p spacetrace-cli every_receipt_here -- --ignored --nocapture`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn every_receipt_here_lists_what_pkgutil_lists() {
        use std::collections::BTreeSet;
        let pkgutil = |args: &[&str]| -> BTreeSet<String> {
            let out = std::process::Command::new("pkgutil")
                .args(args)
                .output()
                .unwrap();
            command_lines(out).into_iter().collect()
        };
        let mut receipts = 0;
        let mut paths = 0;
        let mut longest = 0;
        let mut parse_time = std::time::Duration::ZERO;
        for dir in ["/var/db/receipts", "/Library/Apple/System/Library/Receipts"] {
            let Ok(listing) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in listing {
                let bom = entry.unwrap().path();
                if bom.extension().is_none_or(|e| e != "bom") {
                    continue;
                }
                let info =
                    receipt_info(&std::fs::read(bom.with_extension("plist")).unwrap()).unwrap();
                let id = info.id.unwrap();
                let bytes = std::fs::read(&bom).unwrap();
                let mut all = Collect::default();
                let started = std::time::Instant::now();
                bom_paths(&bytes, &mut all).unwrap_or_else(|e| panic!("{}: {e}", bom.display()));
                parse_time += started.elapsed();
                let all: BTreeSet<String> = all.paths.into_iter().collect();
                assert_eq!(all, pkgutil(&["--files", &id]), "{id}");
                receipts += 1;
                paths += all.len();
                longest = all.iter().map(String::len).max().unwrap_or(0).max(longest);
            }
        }
        eprintln!(
            "{receipts} receipts, {paths} paths, the longest {longest} bytes, \
             parsed in {parse_time:?}"
        );
        assert!(longest <= MAX_PATH_BYTES as usize);
        assert_eq!(receipts, pkgutil(&["--pkgs"]).len());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_writes_what_the_fixture_plist_holds() {
        let dir = tempfile::tempdir().unwrap();
        let plist = dir.path().join("fixture.plist");
        std::fs::write(&plist, FIXTURE_PLIST_XML).unwrap();
        let converted = std::process::Command::new("plutil")
            .args(["-convert", "binary1"])
            .arg(&plist)
            .status()
            .unwrap();
        assert!(converted.success());
        let bytes = std::fs::read(&plist).unwrap();
        assert!(bytes.starts_with(b"bplist00"));
        assert_eq!(receipt_info(&bytes).unwrap(), fixture_info());
    }
}

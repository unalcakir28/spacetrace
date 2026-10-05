//! What the tests of the platform listings share: the ordinary listing's
//! answer to hold them to, and a directory holding everything a real disk
//! does.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::meta::{FileIdentity, RawMeta};

/// What one entry came to: its metadata, or the errno that stopped it.
/// Errors are compared by number, because the listings word nothing
/// themselves and the number is what decides the message.
pub(crate) type Answer = Result<RawMeta, Option<i32>>;

/// Every entry of `dir` as the ordinary walk sees it: `read_dir`, then
/// `lstat` by name. The answer every platform listing is held to.
pub(crate) fn lstat_answers(dir: &Path) -> BTreeMap<OsString, Answer> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let answer = entry
                .metadata()
                .map(|md| {
                    let (meta, failure) =
                        RawMeta::for_path(&entry.path(), &md, FileIdentity::Needed);
                    assert!(failure.is_none());
                    meta
                })
                .map_err(|e| e.raw_os_error());
            (entry.file_name(), answer)
        })
        .collect()
}

pub(crate) fn running_as_root() -> bool {
    // SAFETY: no arguments, no failure mode.
    unsafe { libc::geteuid() == 0 }
}

/// Everything the walk meets on a real disk, in `dir`, which exists. Returns
/// how many entries `dir` itself now holds, so a test can check that every
/// one of them was compared.
///
/// The name that is not UTF-8 is left out on macOS, whose filesystems refuse
/// one (`EILSEQ`); Linux filesystems take any bytes, and that is where the
/// lossily converted name could go wrong.
pub(crate) fn zoo(dir: &Path) -> usize {
    std::fs::write(dir.join("small.txt"), b"hello").unwrap();
    // Not block-aligned, so `size` and `alloc` differ and a path that read
    // one for the other would fail.
    std::fs::write(dir.join("odd.bin"), vec![7u8; 100]).unwrap();
    std::fs::write(dir.join("big.bin"), vec![1u8; 300_000]).unwrap();
    std::fs::write(dir.join("empty"), b"").unwrap();
    // Sparse: 64 MiB long, one block written at the end.
    let sparse = std::fs::File::create(dir.join("sparse.img")).unwrap();
    sparse.set_len(64 << 20).unwrap();
    std::os::unix::fs::FileExt::write_all_at(&sparse, b"tail", (64 << 20) - 4).unwrap();
    drop(sparse);

    std::fs::create_dir(dir.join("sub")).unwrap();
    std::fs::create_dir(dir.join("sub/deeper")).unwrap();
    std::fs::write(dir.join("sub/inside.bin"), vec![0u8; 50_000]).unwrap();

    std::os::unix::fs::symlink("big.bin", dir.join("to-file")).unwrap();
    std::os::unix::fs::symlink("sub", dir.join("to-dir")).unwrap();
    std::os::unix::fs::symlink("/nowhere-at-all", dir.join("broken")).unwrap();

    std::fs::hard_link(dir.join("big.bin"), dir.join("big-again.bin")).unwrap();

    let fifo = std::ffi::CString::new(dir.join("fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: NUL-terminated path; a failure is asserted.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    // Kept bound only for as long as it takes to create the node; the socket
    // file stays behind, which is what the walk would meet.
    drop(std::os::unix::net::UnixListener::bind(dir.join("socket")).unwrap());

    let not_utf8 = std::fs::write(dir.join(OsStr::from_bytes(b"latin1-\xe9t\xe9")), b"x");
    assert!(
        cfg!(target_os = "macos") || not_utf8.is_ok(),
        "{not_utf8:?}"
    );
    std::fs::read_dir(dir).unwrap().count()
}

//! A small disk image attached at a directory: a mount point a test can make
//! without privileges. Shared by the scanner's and the store's rescan tests
//! through `#[path]`, because a mount point is what both have to provoke.
//!
//! macOS only: `hdiutil` attaches an image as an ordinary user. Where it
//! cannot, `attach` says `None` and the caller skips with a message.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Detached when dropped, so a failing assertion does not leave a volume
/// mounted behind it.
pub struct DiskImage {
    mountpoint: PathBuf,
}

impl DiskImage {
    /// A 2 MB HFS+ image created in `scratch` and attached at `mountpoint`,
    /// hidden from the Finder.
    pub fn attach(scratch: &Path, mountpoint: &Path) -> Option<DiskImage> {
        Self::attach_as(scratch, mountpoint, "HFS+", "2m", true)
    }

    /// A 64 MB APFS image attached at `mountpoint` **where the Finder can
    /// see it**, for a few seconds: fseventsd keeps no history for a volume
    /// mounted `-nobrowse` — no events, and `FSEventsCopyUUIDForDevice`
    /// answers nothing (measured) — and a history is what the caller needs.
    #[allow(dead_code)]
    pub fn attach_apfs(scratch: &Path, mountpoint: &Path) -> Option<DiskImage> {
        Self::attach_as(scratch, mountpoint, "APFS", "64m", false)
    }

    fn attach_as(
        scratch: &Path,
        mountpoint: &Path,
        fs: &str,
        size: &str,
        hidden: bool,
    ) -> Option<DiskImage> {
        let image = scratch.join("volume.dmg");
        let quiet = |args: &[&std::ffi::OsStr]| {
            Command::new("hdiutil")
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let created = quiet(&[
            "create".as_ref(),
            "-size".as_ref(),
            size.as_ref(),
            "-fs".as_ref(),
            fs.as_ref(),
            "-volname".as_ref(),
            "spacetrace-test".as_ref(),
            image.as_os_str(),
        ]);
        let mut attach: Vec<&std::ffi::OsStr> = vec!["attach".as_ref()];
        if hidden {
            attach.push("-nobrowse".as_ref());
        }
        attach.extend::<[&std::ffi::OsStr; 3]>([
            "-mountpoint".as_ref(),
            mountpoint.as_os_str(),
            image.as_os_str(),
        ]);
        let attached = created && quiet(&attach);
        attached.then(|| DiskImage {
            mountpoint: mountpoint.to_path_buf(),
        })
    }
}

impl Drop for DiskImage {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .arg("detach")
            .arg("-force")
            .arg(&self.mountpoint)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

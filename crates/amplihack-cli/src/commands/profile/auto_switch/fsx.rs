//! File helpers for auto-switch state: bounded no-follow reads, private
//! atomic writes, directory syncs and ownership checks. Every read in the
//! auto-switch modules goes through `read_bounded_nofollow` or
//! `read_bounded`, so no file or pipe can make amplihack allocate without
//! limit.
//!
//! The #1556 helpers in `store.rs` and `pointer.rs` are reused where they fit
//! (`private_dir_builder`, `rename_noreplace`, `check_root_not_symlink`). The rest are not: they return
//! `ProfileError`, have best-effort non-Unix fallbacks, and do not refuse a
//! file owned by another user or sync the parent directory, all of which the
//! credential moves here rely on.
//!
//! The generic helpers (`read_bounded_nofollow`, `write_private_atomic`,
//! `ensure_private_dir`, …) live in `amplihack_utils::secure_files` since
//! issue #1658, so the limit-resume hook shares one implementation; they are
//! re-exported here unchanged.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub use amplihack_utils::secure_files::{
    check_private_dir, ensure_private_dir, euid, fsync_dir, invalid, read_bounded,
    read_bounded_nofollow, writable_by_others, write_atomic_keep_mode, write_private_atomic,
};

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// True when `path` exists (without following a symlink).
pub fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// A login file amplihack may move: a regular file owned by the current user,
/// readable by nobody else, with exactly one hard link. Only metadata is
/// looked at. The mode check matters because a login hidden by a private
/// profile directory would otherwise become readable once moved into a
/// group-readable `~/.claude`.
pub fn movable_login(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|m| m.is_file() && m.uid() == euid() && m.mode() & 0o077 == 0 && m.nlink() == 1)
}

/// Sets the modification time to now without following a symlink, so Claude
/// notices the file changed.
pub fn touch_nofollow(path: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: valid NUL-terminated path; a null `times` means "now".
    let rc = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            std::ptr::null(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The root may hold auto-switch state only if it is a real directory owned
/// by the current user that nobody else can write to. The symlink check is
/// the profile store's own (#1556).
pub fn check_root_private(root: &Path) -> io::Result<()> {
    crate::commands::profile::store::check_root_not_symlink(root)
        .map_err(|e| invalid(&e.to_string()))?;
    let m = fs::symlink_metadata(root)?;
    if !m.is_dir() {
        return Err(invalid("is not a real directory"));
    }
    if m.uid() != euid() {
        return Err(invalid("is owned by another user"));
    }
    if writable_by_others(&m) {
        return Err(invalid("is writable by other users"));
    }
    Ok(())
}

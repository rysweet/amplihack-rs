//! Primitives that move a login file, shared by auto-switch and `profile
//! rotate` (issue #1592). A login file is only ever renamed, never opened:
//! with `RENAME_NOREPLACE`, or exchanged with another login atomically.
//! Claude's refresh lock is taken and released by path and inode.

use super::fsx;
use crate::commands::profile::store::rename_noreplace;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const REFRESH_LOCK: &str = ".oauth_refresh.lock";

#[cfg(debug_assertions)]
const TEST_ABORT_ENV: &str = "AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER";

/// Debug builds only: stop the process at a named step, as a crash would.
pub fn abort_point(step: &str) {
    #[cfg(debug_assertions)]
    if std::env::var_os(TEST_ABORT_ENV).is_some_and(|v| v == step) {
        std::process::exit(86);
    }
    let _ = step;
}

pub enum MoveError {
    /// The destination exists: two copies, and amplihack cannot tell which is
    /// current.
    Exists,
    /// The filesystem or platform cannot rename between these paths.
    Unsupported,
    Other(String),
}

fn unsupported(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EXDEV | libc::EINVAL | libc::ENOSYS)
    )
}

/// Syncs the directories holding `a` and `b` after a rename.
fn sync_parents(a: &Path, b: &Path) -> std::io::Result<()> {
    let pa = a.parent().unwrap_or(Path::new("."));
    fsx::fsync_dir(pa)?;
    match b.parent() {
        Some(pb) if pb != pa => fsx::fsync_dir(pb),
        _ => Ok(()),
    }
}

/// Renames one login file. The source must be a regular file owned by the
/// current user with one hard link; the destination must not exist. `show`
/// turns a path into the text used in messages.
pub fn move_login(src: &Path, dst: &Path, show: &dyn Fn(&Path) -> String) -> Result<(), MoveError> {
    if !fsx::movable_login(src) {
        return Err(MoveError::Other(format!(
            "{} is not a private regular file with one link",
            show(src)
        )));
    }
    match rename_noreplace(src, dst) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => return Err(MoveError::Exists),
        Err(e) if unsupported(&e) => return Err(MoveError::Unsupported),
        Err(e) => return Err(MoveError::Other(format!("{}: {e}", show(src)))),
    }
    sync_parents(dst, src).map_err(|e| MoveError::Other(format!("sync after move: {e}")))
}

/// Exchanges two login files in one atomic step, so each path holds a login
/// throughout. Both must pass [`fsx::movable_login`]. Only Linux has the
/// call; anywhere else, and on a filesystem that refuses it, the result is
/// `Unsupported` and nothing has moved. There is no copy fallback.
pub fn exchange_logins(a: &Path, b: &Path) -> Result<(), MoveError> {
    if !fsx::movable_login(a) || !fsx::movable_login(b) {
        return Err(MoveError::Other(
            "a login file is not a private regular file with one link".into(),
        ));
    }
    exchange(a, b).map_err(|e| {
        if unsupported(&e) {
            MoveError::Unsupported
        } else {
            MoveError::Other(format!("exchange failed: {}", e.kind()))
        }
    })?;
    sync_parents(a, b).map_err(|e| MoveError::Other(format!("sync after exchange: {e}")))
}

#[cfg(target_os = "linux")]
fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let cstr = |p: &Path| {
        std::ffi::CString::new(p.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    };
    let (a, b) = (cstr(a)?, cstr(b)?);
    // SAFETY: both paths are valid NUL-terminated strings; RENAME_EXCHANGE
    // swaps two existing names atomically and creates or removes nothing.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn exchange(_a: &Path, _b: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
}

/// Takes `<dir>/.oauth_refresh.lock` with a non-blocking `mkdir`, as Claude
/// does, and returns its inode. `None` when it is held: the lock is never
/// broken.
pub fn take_refresh_lock_at(dir: &Path) -> Option<u64> {
    let path = dir.join(REFRESH_LOCK);
    let mut b = fs::DirBuilder::new();
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(&path).ok()?;
    fs::symlink_metadata(&path).ok().map(|m| m.ino())
}

/// Removes the refresh lock only if it is a directory with inode `ino`, the
/// one amplihack created. Returns whether it was removed.
pub fn release_refresh_lock_at(dir: &Path, ino: u64) -> bool {
    let path = dir.join(REFRESH_LOCK);
    fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir() && m.ino() == ino)
        && fs::remove_dir(&path).is_ok()
}

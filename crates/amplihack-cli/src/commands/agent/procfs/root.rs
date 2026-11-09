//! Where the process table is read (S27, issue #2042): `/proc`, or the fake
//! tree a test names in `AMPLIHACK_TEST_PROC_ROOT`.
//!
//! [`proc_root`] is the one resolver. Every reader of the process table
//! calls it, and #2040 must too; nothing else reads the variable. Release
//! builds honour it on purpose (SR-2): the seam needs the test home guard
//! and a directory under the temp dir that this user owns and others cannot
//! write, so whoever can meet those conditions already acts as this user.
//!
//! Some readers stay on the real `/proc` on purpose: `/proc/loadavg`, the
//! process's own `/proc/self/*`, the process-tree kill in `util.rs`,
//! `rotator_down`, `running_on_host` (the check `rotate stop` makes just
//! before it signals), and `recipe/process_tree.rs`, which `recipe stop` and
//! `recipe run`'s teardown use to find the processes they then signal (see
//! the guide). A fake tree must never choose a pid that is signalled.
//!
//! A refused variable falls back to `/proc`. Debug builds, which every test
//! runs, print one stderr line naming the reason, so a child that reads the
//! host table by mistake shows it in its output.

use amplihack_utils::secure_files::{euid, writable_by_others};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const VAR: &str = "AMPLIHACK_TEST_PROC_ROOT";

/// Why the seam refused the variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The test home guard is unset or empty.
    NoGuard,
    /// The path does not exist or cannot be resolved.
    Missing,
    /// The resolved path is not strictly inside the resolved temp dir.
    OutsideTemp,
    /// The path is not a directory.
    NotADirectory,
    /// Another user owns the directory.
    WrongOwner,
    /// Others can write the directory.
    WritableByOthers,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::NoGuard => "the test home guard is not set",
            Refusal::Missing => "the path does not exist",
            Refusal::OutsideTemp => "the path is not inside the temp dir",
            Refusal::NotADirectory => "the path is not a directory",
            Refusal::WrongOwner => "another user owns the directory",
            Refusal::WritableByOthers => "others can write the directory",
        })
    }
}

/// `/proc`, or the test's fake tree when [`resolve`] accepts it.
pub fn proc_root() -> PathBuf {
    let var = std::env::var_os(VAR);
    let guard = std::env::var_os(crate::user_config::TEST_HOME_GUARD_ENV);
    match resolve(var.as_deref(), guard.as_deref(), &std::env::temp_dir()) {
        Ok(Some(root)) => root,
        Ok(None) => PathBuf::from("/proc"),
        Err(why) => {
            warn_once(why);
            PathBuf::from("/proc")
        }
    }
}

/// The pure part of [`proc_root`]. `Ok(None)` when `var` is unset or
/// empty, `Ok(Some(root))` when the seam is honoured, and the reason
/// otherwise. The seam is honoured only under a non-empty test guard, for a path whose canonical form lies strictly
/// inside the canonical temp dir, and for a directory owned by this user
/// that others cannot write (S27).
pub fn resolve(
    var: Option<&OsStr>,
    guard: Option<&OsStr>,
    temp_dir: &Path,
) -> Result<Option<PathBuf>, Refusal> {
    let Some(var) = var.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if !guard.is_some_and(|g| !g.is_empty()) {
        return Err(Refusal::NoGuard);
    }
    let root = Path::new(var)
        .canonicalize()
        .map_err(|_| Refusal::Missing)?;
    let temp = temp_dir.canonicalize().map_err(|_| Refusal::OutsideTemp)?;
    if !root.starts_with(&temp) || root == temp {
        return Err(Refusal::OutsideTemp);
    }
    let meta = fs::metadata(&root).map_err(|_| Refusal::Missing)?;
    if !meta.is_dir() {
        Err(Refusal::NotADirectory)
    } else if meta.uid() != euid() {
        Err(Refusal::WrongOwner)
    } else if writable_by_others(&meta) {
        Err(Refusal::WritableByOthers)
    } else {
        Ok(Some(root))
    }
}

/// One stderr line per process, in debug builds only; release behaviour is
/// unchanged.
fn warn_once(why: Refusal) {
    #[cfg(debug_assertions)]
    {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| eprintln!("amplihack: {VAR} ignored ({why}); reading /proc"));
    }
    #[cfg(not(debug_assertions))]
    let _ = why;
}

#[cfg(test)]
#[path = "tests_root.rs"]
mod tests;

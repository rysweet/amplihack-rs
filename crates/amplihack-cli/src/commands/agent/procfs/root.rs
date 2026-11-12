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
//!
//! Lib tests (issue #2196) never set the variable. They install a fake tree
//! for the calling thread only with `override_proc_root`, which exists
//! under `cfg(test)` alone. The override runs the same `check_tree` as the
//! variable, without the home guard, and a refused tree panics instead of
//! falling back. With no override set, an accepted variable is used, and every
//! other outcome (unset or refused) gives one shared fake table per test
//! process (`test_support::proc::shared_default_root`), never `/proc`. That
//! step is compiled only under `cfg(test)`; other builds fall back to `/proc`.

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

/// `/proc`, or the test's fake tree when [`resolve`] accepts it. In lib
/// tests, the calling thread's `override_proc_root` tree comes first.
pub fn proc_root() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = OVERRIDE.with(|o| o.borrow().clone()) {
        return root;
    }
    let var = std::env::var_os(VAR);
    let guard = std::env::var_os(crate::user_config::TEST_HOME_GUARD_ENV);
    root_from(resolve(
        var.as_deref(),
        guard.as_deref(),
        &std::env::temp_dir(),
    ))
}

/// The root for an outcome of [`resolve`]: the accepted tree, or else the
/// fallback. The fallback is `/proc` in other builds and the shared fake
/// table in lib tests (issue #2196), so a lib test never reads the host
/// table through the seam. A refused variable also warns once.
fn root_from(outcome: Result<Option<PathBuf>, Refusal>) -> PathBuf {
    match outcome {
        Ok(Some(root)) => return root,
        Ok(None) => {}
        Err(why) => warn_once(why),
    }
    fallback_root()
}

#[cfg(not(test))]
fn fallback_root() -> PathBuf {
    PathBuf::from("/proc")
}

#[cfg(test)]
fn fallback_root() -> PathBuf {
    crate::test_support::proc::shared_default_root().to_path_buf()
}

/// What [`warn_once`] says the process reads instead of a refused variable.
#[cfg(not(test))]
#[cfg_attr(not(debug_assertions), allow(dead_code))]
const FALLBACK: &str = "reading /proc";
#[cfg(test)]
const FALLBACK: &str = "reading the shared fake table";

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
    check_tree(var, temp_dir).map(Some)
}

/// The tree checks of [`resolve`], after the guard: `var` resolves to a
/// directory strictly inside the canonical `temp_dir`, owned by this user
/// and not writable by others. Returns the canonical path.
fn check_tree(var: &OsStr, temp_dir: &Path) -> Result<PathBuf, Refusal> {
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
        Ok(root)
    }
}

/// The canonical form of the fake tree `root` when it passes
/// [`check_tree`] against the process temp dir. Panics naming `caller`, the
/// path and the reason otherwise: a test tree that fails the checks is a
/// broken fixture, never a reason to read `/proc`.
#[cfg(test)]
pub fn checked_fake_tree(caller: &str, root: &Path) -> PathBuf {
    check_tree(root.as_os_str(), &std::env::temp_dir())
        .unwrap_or_else(|why| panic!("{caller}({}): {why}", root.display()))
}

#[cfg(test)]
thread_local! {
    /// The calling thread's fake tree, set only by [`override_proc_root`].
    static OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Restores the thread's previous override when dropped. Not `Send`: the
/// override belongs to the thread that installed it, so the guard must drop
/// there. It is `Sync`, so a test can still lend a fixture holding one to a
/// scoped thread, which installs its own. Guards must drop in reverse order
/// of installation, which plain scoping gives.
#[cfg(test)]
#[must_use = "the override is removed as soon as the guard drops"]
pub struct ProcRootOverride {
    previous: Option<PathBuf>,
    _not_send: std::marker::PhantomData<std::sync::MutexGuard<'static, ()>>,
}

#[cfg(test)]
impl Drop for ProcRootOverride {
    fn drop(&mut self) {
        let previous = self.previous.take();
        OVERRIDE.with(|o| *o.borrow_mut() = previous);
    }
}

/// Make [`proc_root`] return `root` on this thread until the guard drops
/// (issue #2196). `root` must pass the same checks as the variable, apart
/// from the home guard; a refused tree panics with the path and the reason,
/// and never falls back to `/proc`.
#[cfg(test)]
pub fn override_proc_root(root: &Path) -> ProcRootOverride {
    let root = checked_fake_tree("override_proc_root", root);
    let previous = OVERRIDE.with(|o| o.borrow_mut().replace(root));
    ProcRootOverride {
        previous,
        _not_send: std::marker::PhantomData,
    }
}

/// One stderr line per process, in debug builds only; release behaviour is
/// unchanged.
fn warn_once(why: Refusal) {
    #[cfg(debug_assertions)]
    {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| eprintln!("amplihack: {VAR} ignored ({why}); {FALLBACK}"));
    }
    #[cfg(not(debug_assertions))]
    let _ = why;
}

#[cfg(test)]
#[path = "tests_root.rs"]
mod tests;

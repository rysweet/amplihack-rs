//! Which folders running processes use (issue #1808): working folders, open
//! files and TMPDIR/TMP/TEMP read from the process table. Split out of
//! `sweep.rs`; also used by the run's runtime guard (issue #2486).
//!
//! The host's table is `procfs::readable_table()` (issues #2042, #2935), so
//! a test that runs `amplihack cleanup` reads its own fake tree, never every
//! process on the host. Outside tests that is `/proc`, and a system without
//! `/proc` has no table: every folder is then of unknown use. Nothing here
//! signals a process: a fake tree can only make a folder look free or busy,
//! and the folders a test's cleanup can reach are that test's own temp
//! folders.

use std::fs;
use std::path::{Path, PathBuf};

/// The view of what running processes hold (issue #2935): the working
/// folder, every open file and the TMPDIR/TMP/TEMP values of each process of
/// this user other than this one. `None` when this system gives no such
/// view. The code that decides whether a folder may go takes one of these,
/// so a test of that decision writes its own table and starts no process.
pub(crate) trait ProcessView: Send + Sync {
    fn held_paths(&self) -> Option<Vec<PathBuf>>;
}

/// The host's view: the process table `procfs::readable_table()` names,
/// which is `/proc` on Linux. A system without `/proc` has none.
pub(crate) struct HostProcesses;

impl ProcessView for HostProcesses {
    fn held_paths(&self) -> Option<Vec<PathBuf>> {
        let table = crate::commands::agent::procfs::readable_table()?;
        let entries = fs::read_dir(table).ok()?;
        let me = std::process::id().to_string();
        let mut paths = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == me || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let base = entry.path();
            if let Ok(cwd) = fs::read_link(base.join("cwd")) {
                paths.push(cwd);
            }
            // A child of a killed run may have nothing open in its TMPDIR
            // between writes; its environment still points there.
            if let Ok(environ) = fs::read(base.join("environ")) {
                for var in environ.split(|b| *b == 0) {
                    for key in [&b"TMPDIR="[..], b"TMP=", b"TEMP="] {
                        if let Some(value) = var.strip_prefix(key)
                            && value.first() == Some(&b'/')
                        {
                            use std::os::unix::ffi::OsStrExt;
                            paths.push(PathBuf::from(std::ffi::OsStr::from_bytes(value)));
                        }
                    }
                }
            }
            if let Ok(fds) = fs::read_dir(base.join("fd")) {
                for fd in fds.flatten() {
                    if let Ok(target) = fs::read_link(fd.path())
                        && target.is_absolute()
                    {
                        paths.push(target);
                    }
                }
            }
        }
        Some(paths)
    }
}

/// A table a test holds in memory. `hold` and `release` change it while a
/// folder's owner keeps the view, as a process starting or exiting would.
#[cfg(test)]
pub(crate) struct FakeProcesses(std::sync::Mutex<Option<Vec<PathBuf>>>);

#[cfg(test)]
impl FakeProcesses {
    /// Processes can be seen, and none holds anything yet.
    pub(crate) fn seen() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(std::sync::Mutex::new(Some(Vec::new()))))
    }

    /// The answer of a system where processes cannot be seen.
    pub(crate) fn unseen() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(std::sync::Mutex::new(None)))
    }

    /// Adds one held path: a working folder, an open file or a TMPDIR value.
    pub(crate) fn hold(&self, path: &Path) {
        self.0
            .lock()
            .unwrap()
            .as_mut()
            .expect("an unseen table holds nothing")
            .push(path.to_path_buf());
    }

    /// Drops every held path, as when the holding processes have exited.
    pub(crate) fn release(&self) {
        if let Some(paths) = self.0.lock().unwrap().as_mut() {
            paths.clear();
        }
    }
}

#[cfg(test)]
impl ProcessView for FakeProcesses {
    fn held_paths(&self) -> Option<Vec<PathBuf>> {
        self.0.lock().unwrap().clone()
    }
}

/// What one scan says about a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Use {
    /// A process has its working folder, an open file or its temporary
    /// folder variable in it.
    Held,
    /// The scan saw the processes and none uses it.
    Free,
    /// Processes cannot be seen here, so nothing is known.
    Unknown,
}

/// Working folders and open files of every other process this user can see.
pub(crate) struct ProcScan {
    available: bool,
    paths: Vec<PathBuf>,
}

impl ProcScan {
    /// One scan of the host's view.
    pub(crate) fn take() -> Self {
        Self::of(&HostProcesses)
    }

    /// One scan of `view`.
    pub(crate) fn of(view: &dyn ProcessView) -> Self {
        match view.held_paths() {
            Some(paths) => Self {
                available: true,
                paths,
            },
            None => Self {
                available: false,
                paths: Vec::new(),
            },
        }
    }

    /// Whether a process has its working folder, a file open or its temporary
    /// folder variable in `path`.
    /// Without a view of the processes, everything counts as in use.
    pub(crate) fn in_use(&self, path: &Path) -> bool {
        self.judge(path) != Use::Free
    }

    /// [`Self::in_use`] with "cannot tell" kept apart (issue #2935), for a
    /// caller that says which of the two it found.
    pub(crate) fn judge(&self, path: &Path) -> Use {
        if !self.available {
            return Use::Unknown;
        }
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let held = self
            .paths
            .iter()
            .any(|p| p.starts_with(&canonical) || p.starts_with(path));
        if held { Use::Held } else { Use::Free }
    }
}

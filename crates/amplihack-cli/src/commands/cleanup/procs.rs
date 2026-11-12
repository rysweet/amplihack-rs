//! Which folders running processes use (issue #1808): working folders, open
//! files and TMPDIR/TMP/TEMP read from the process table. Split out of
//! `sweep.rs`.
//!
//! The table is `procfs::proc_root()` (issue #2042), so a test that runs
//! `amplihack cleanup` reads its own fake tree, never every process on the
//! host. Outside tests that is `/proc`. Nothing here signals a process: a
//! fake tree can only make a folder look free or busy, and the folders a
//! test's cleanup can reach are that test's own temp folders.

use std::fs;
use std::path::{Path, PathBuf};

/// Working folders and open files of every other process this user can see.
pub(super) struct ProcScan {
    available: bool,
    paths: Vec<PathBuf>,
}

impl ProcScan {
    pub(super) fn take() -> Self {
        let Ok(entries) = fs::read_dir(crate::commands::agent::procfs::proc_root()) else {
            return Self {
                available: false,
                paths: Vec::new(),
            };
        };
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
        Self {
            available: true,
            paths,
        }
    }

    /// Whether a process has its working folder, a file open or its temporary
    /// folder variable in `path`.
    /// Without `/proc`, everything counts as in use.
    pub(super) fn in_use(&self, path: &Path) -> bool {
        if !self.available {
            return true;
        }
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.paths
            .iter()
            .any(|p| p.starts_with(&canonical) || p.starts_with(path))
    }
}

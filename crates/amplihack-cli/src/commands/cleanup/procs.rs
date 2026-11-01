//! Which folders running processes use (issue #1808): working folders and
//! open files read from `/proc`. Moved out of `sweep.rs` unchanged.

use std::fs;
use std::path::{Path, PathBuf};

/// Working folders and open files of every other process this user can see.
pub(super) struct ProcScan {
    available: bool,
    paths: Vec<PathBuf>,
}

impl ProcScan {
    pub(super) fn take() -> Self {
        let Ok(entries) = fs::read_dir("/proc") else {
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

    /// Whether a process has its working folder in `path` or a file open in it.
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

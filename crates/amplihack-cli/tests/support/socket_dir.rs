//! Directories for test Unix sockets that fit in `sun_path`.
//!
//! A socket path must fit in `sun_path`, 108 bytes on Linux including the
//! terminating NUL (unix(7)). A nested workflow can set a `TMPDIR` over 100
//! bytes long, which leaves no room for `tmux-<uid>/default` under a temp
//! directory. The rule every test follows:
//!
//! 1. Measure the real socket path; use the directory under `TMPDIR` when it
//!    fits.
//! 2. Otherwise make a private directory under `XDG_RUNTIME_DIR`, the
//!    per-user directory meant for sockets, when the path fits there.
//! 3. Otherwise use a fresh owner-only directory under the canonical `/tmp`.
//!    The returned TempDir guard removes it when the fixture ends.
//! 4. If no candidate fits, report the limit.
//!
//! Used by the `cli_integration` binary (`support::socket_dir`) and, through
//! `#[path]`, by the unit tests in `src/commands/agent/tests_socket_check.rs`,
//! so both follow the one rule.

#![allow(dead_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Bytes in `sun_path` (unix(7)), including the terminating NUL.
pub const SUN_PATH_MAX: usize = 108;

/// Characters `tempfile` adds for a directory made with prefix `t`: the
/// prefix and a six-character random suffix.
const TEMPDIR_NAME: &str = "t000000";

/// Whether `path` fits in `sun_path`.
pub fn fits(path: &Path) -> bool {
    path.as_os_str().len() < SUN_PATH_MAX
}

/// `tmux-<uid>/default`, the tail of tmux's socket under `TMUX_TMPDIR`.
pub fn tmux_tail() -> PathBuf {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    Path::new(&format!("tmux-{uid}")).join("default")
}

/// Whether tmux's socket, `<dir>/tmux-<uid>/default`, fits in `sun_path`.
pub fn tmux_socket_fits(dir: &Path) -> bool {
    fits(&dir.join(tmux_tail()))
}

/// A private short directory, preferring `XDG_RUNTIME_DIR` to `/tmp`.
/// Neither candidate is used when its canonical socket path exceeds the limit.
pub fn runtime_dir_for(tail: &Path) -> Option<tempfile::TempDir> {
    let candidates = [
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        Some(PathBuf::from("/tmp")),
    ];
    for candidate in candidates.into_iter().flatten() {
        let Ok(runtime) = candidate.canonicalize() else {
            continue;
        };
        if !runtime.is_dir() || !fits(&runtime.join(TEMPDIR_NAME).join(tail)) {
            continue;
        }
        if let Ok(dir) = tempfile::Builder::new()
            .prefix("t")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(&runtime)
        {
            return Some(dir);
        }
    }
    None
}

/// The directory in which to put a socket at `<dir>/<tail>`.
///
/// `local` when the path fits there; otherwise a new directory under
/// `XDG_RUNTIME_DIR` or `/tmp`, returned with its guard. When none fits, `local` is
/// returned and the caller must check [`fits`] before binding.
pub fn socket_dir(local: PathBuf, tail: &Path) -> (PathBuf, Option<tempfile::TempDir>) {
    if fits(&local.join(tail)) {
        return (local, None);
    }
    match runtime_dir_for(tail) {
        Some(dir) => (dir.path().to_path_buf(), Some(dir)),
        None => (local, None),
    }
}

/// The directory to use as `TMUX_TMPDIR`: `<root>/t`, or one under
/// `XDG_RUNTIME_DIR` or `/tmp` when tmux's socket does not fit under `root`.
pub fn tmux_socket_dir(root: &Path) -> (PathBuf, Option<tempfile::TempDir>) {
    tmux_socket_dir_named(root, "t")
}

/// As [`tmux_socket_dir`], with `<root>/<name>` as the local choice.
pub fn tmux_socket_dir_named(root: &Path, name: &str) -> (PathBuf, Option<tempfile::TempDir>) {
    socket_dir(root.join(name), &tmux_tail())
}

/// A new temp directory in which a socket at `<dir>/<tail>` fits: under
/// `TMPDIR` when it fits there, else under `XDG_RUNTIME_DIR` or `/tmp`.
/// `None` when none fits.
pub fn socket_tempdir(tail: &Path) -> Option<tempfile::TempDir> {
    let tmp = tempfile::Builder::new().prefix("t").tempdir().ok()?;
    if fits(&tmp.path().join(tail)) {
        return Some(tmp);
    }
    runtime_dir_for(tail)
}

/// The message a test prints when it skips because `path` is too long.
pub fn skip_message(path: &Path) -> String {
    format!(
        "skipping: the socket {} would exceed the {SUN_PATH_MAX}-byte sun_path limit \
         and no short private socket directory is available; set XDG_RUNTIME_DIR or use a shorter TMPDIR",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_workflow_temp_paths_get_a_private_short_socket_directory() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("x".repeat(SUN_PATH_MAX));
        let (path, guard) = socket_dir(local.clone(), &tmux_tail());
        let guard = guard.expect("long TMPDIR must have a short socket fallback");
        assert_ne!(path, local);
        assert!(tmux_socket_fits(&path));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        drop(guard);
        assert!(
            !path.exists(),
            "the fixture must remove its socket directory"
        );
    }
}

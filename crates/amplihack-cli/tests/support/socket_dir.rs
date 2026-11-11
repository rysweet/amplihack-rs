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
//! 3. Otherwise report that nothing fits; the caller skips, or stops with a
//!    message naming the limit.
//!
//! The tests never write to `/tmp`.
//!
//! Used by the `cli_integration` binary (`support::socket_dir`) and, through
//! `#[path]`, by the unit tests in `src/commands/agent/tests_socket_check.rs`,
//! so both follow the one rule.

#![allow(dead_code)]

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

/// A new private directory under `XDG_RUNTIME_DIR` in which `tail` fits, or
/// `None` when that variable is unset, is not a directory, or is too long.
pub fn runtime_dir_for(tail: &Path) -> Option<tempfile::TempDir> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?)
        .canonicalize()
        .ok()?;
    if !runtime.is_dir() || !fits(&runtime.join(TEMPDIR_NAME).join(tail)) {
        return None;
    }
    tempfile::Builder::new()
        .prefix("t")
        .tempdir_in(&runtime)
        .ok()
}

/// The directory in which to put a socket at `<dir>/<tail>`.
///
/// `local` when the path fits there; otherwise a new directory under
/// `XDG_RUNTIME_DIR`, returned with its guard. When neither fits, `local` is
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
/// `XDG_RUNTIME_DIR` when tmux's socket does not fit under `root`.
pub fn tmux_socket_dir(root: &Path) -> (PathBuf, Option<tempfile::TempDir>) {
    tmux_socket_dir_named(root, "t")
}

/// As [`tmux_socket_dir`], with `<root>/<name>` as the local choice.
pub fn tmux_socket_dir_named(root: &Path, name: &str) -> (PathBuf, Option<tempfile::TempDir>) {
    socket_dir(root.join(name), &tmux_tail())
}

/// A new temp directory in which a socket at `<dir>/<tail>` fits: under
/// `TMPDIR` when it fits there, else under `XDG_RUNTIME_DIR`. `None` when
/// neither fits.
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
         and XDG_RUNTIME_DIR offers no shorter directory; run with a shorter TMPDIR",
        path.display()
    )
}

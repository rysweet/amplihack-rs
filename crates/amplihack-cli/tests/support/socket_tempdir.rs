//! A temp dir short enough to hold a Unix socket path.
//!
//! The one copy of this helper. The `cli_integration` binary reaches it as
//! `support::socket_tempdir`; the lib's `agent::tests_socket_check` unit
//! tests include this file with `#[path]`, because a lib unit test cannot
//! see the integration-test tree and an integration test cannot see a
//! `#[cfg(test)]` module of the lib. It depends only on `std` and `tempfile`.
//!
//! The limit: `sun_path` is 108 bytes on Linux including the NUL, so a
//! socket path may be at most 107 bytes (unix(7)). A nested workflow run
//! sets a `$TMPDIR` over 100 bytes, which leaves no room for
//! `<tempdir>/tt/tmux-<uid>/default`. Instead of guessing a safe `$TMPDIR`
//! length, the helper creates the dir, measures the real, canonical path
//! plus the deepest path the caller will use under it, and falls back to
//! `/tmp` only when that sum is over the limit.

#![allow(dead_code)]

use std::path::Path;

/// Longest socket path Linux accepts: `sun_path` (108) minus the NUL.
pub const SUN_PATH_MAX: usize = 107;

/// Bytes `dir` plus `/` plus `deepest` takes once `dir` is canonicalised.
fn socket_path_len(dir: &Path, deepest: &str) -> usize {
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    canonical.as_os_str().len() + 1 + deepest.len()
}

/// A temp dir under `$TMPDIR` if `<dir>/<deepest>` fits in
/// [`SUN_PATH_MAX`] bytes there, otherwise under `/tmp`.
///
/// `deepest` is the longest path, relative to the returned dir, that the
/// caller will bind or check a socket at, for example
/// `tt/tmux-1000/default`. Panics if even `/tmp` leaves too little room,
/// so a test fails with the reason instead of an `EINVAL` from `bind`.
pub fn socket_tempdir(deepest: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    if socket_path_len(tmp.path(), deepest) <= SUN_PATH_MAX {
        return tmp;
    }
    drop(tmp);
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let len = socket_path_len(tmp.path(), deepest);
    assert!(
        len <= SUN_PATH_MAX,
        "{}/{deepest} is {len} bytes, over the {SUN_PATH_MAX}-byte socket limit even under /tmp",
        tmp.path().display()
    );
    tmp
}

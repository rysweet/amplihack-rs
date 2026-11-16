//! Write executable stubs without this process holding a write descriptor.
//!
//! Exec of a file fails with ETXTBSY while any process holds it open for
//! writing. If this process wrote a stub itself, a sibling test that forks
//! during the write would inherit the descriptor and keep it until that
//! child execs, and a concurrent exec of the stub would fail. Every module
//! of `cli_integration` shares one process, so that window is real. Here a
//! short-lived `sh` writes the file instead. This process never opens it for
//! writing, so no fork of ours can inherit such a descriptor, whoever later
//! execs the stub.
//!
//! The `sh` writes a temporary name (`<path>.stub-tmp.<pid of sh>`), sets the
//! mode and renames it over `path`. A test that rewrites a stub while an
//! earlier copy is still running therefore replaces the directory entry
//! instead of opening the running file, which would fail with ETXTBSY.
//!
//! The children run with an explicit `PATH` (`STUB_WRITER_PATH`) instead of
//! the inherited one. Env-mutating tests in these binaries set the
//! process-wide `PATH` to a scratch directory while they run, and a stub
//! write that happened to overlap one of them resolved no `cat`, `chmod` or
//! `mv` and died with status 127 (issue #2019).
//!
//! This file uses only `std`. `amplihack-cli`'s and `amplihack-hooks`'s
//! `test_support` modules and the `integration` target include it with
//! `#[path]`, so there is one stub writer.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// `PATH` for the stub-writing children. Fixed, so a concurrent test that
/// replaces the process-wide `PATH` cannot take `cat`, `chmod` or `mv` away
/// from them. It matches the absolute `/bin/sh` and `/bin/cp` below.
const STUB_WRITER_PATH: &str = "/usr/bin:/bin";

/// Writes `body` to `path` (replacing it by rename) with permission bits
/// `mode`. The path and mode reach `sh` as arguments, never as script text.
///
/// Panics before writing anything if `mode` has a setuid, setgid or sticky
/// bit (`mode & 0o7000 != 0`): a test stub never needs one.
pub fn write_executable(path: &Path, body: impl AsRef<[u8]>, mode: u32) {
    assert!(
        mode & 0o7000 == 0,
        "refusing to write stub {} with mode {mode:o}: setuid, setgid and sticky \
         bits (0o7000) are not allowed",
        path.display()
    );
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            "t=\"$1.stub-tmp.$$\"; cat > \"$t\" && chmod \"$2\" \"$t\" && mv -f \"$t\" \"$1\"",
            "sh",
        ])
        .arg(path)
        .arg(format!("{mode:o}"))
        .env("PATH", STUB_WRITER_PATH)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start sh to write {}: {e}", path.display()));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(body.as_ref())
        .unwrap_or_else(|e| panic!("cannot stream {}: {e}", path.display()));
    let status = child.wait().expect("wait for sh");
    assert!(
        status.success(),
        "writing {} failed: {status}",
        path.display()
    );
}

/// Copies `from` to `to`, mode bits included, through a `cp` child, for the
/// same reason as `write_executable`: `fs::copy` would open `to` for writing
/// in this process.
pub fn copy_executable(from: &Path, to: &Path) {
    let status = Command::new("/bin/cp")
        .arg("--")
        .arg(from)
        .arg(to)
        .env("PATH", STUB_WRITER_PATH)
        .status()
        .unwrap_or_else(|e| panic!("cannot start cp for {}: {e}", to.display()));
    assert!(
        status.success(),
        "copying {} to {} failed: {status}",
        from.display(),
        to.display()
    );
}

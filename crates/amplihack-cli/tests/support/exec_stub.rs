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

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes `body` to `path` (replacing it) and sets permission bits `mode`.
pub fn write_executable(path: &Path, body: impl AsRef<[u8]>, mode: u32) {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod \"$2\" \"$1\"", "sh"])
        .arg(path)
        .arg(format!("{mode:o}"))
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
        .status()
        .unwrap_or_else(|e| panic!("cannot start cp for {}: {e}", to.display()));
    assert!(
        status.success(),
        "copying {} to {} failed: {status}",
        from.display(),
        to.display()
    );
}
